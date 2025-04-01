/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
use crate::engine::job::{DownloadJob, JobState};
use crate::engine::manager::ManagerEvent;
use crate::engine::manager::gossip::PROVISION_EXPIRATION;
use crate::engine::protocol::{BlobsPacket, SyncifyConnection, SyncifyPacket};
use crate::store::StoreManager;
use crate::{SharedDirectory, get_app_cache_dir};
use blake3::Hash;
use chrono::{DateTime, Utc};
use iroh::Endpoint;
use iroh_base::NodeId;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::ops::{Add, Deref};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Size of the event buffer for [`Downloader`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Size of the chunk of file that are sent per packet.
pub const CHUNK_SIZE: usize = 16 * 1024;
/// Number of concurrent download threads
pub const MAX_DOWNLOAD_TASKS: usize = 10;

pub struct DownloadTask {
    handle: JoinHandle<()>,
}

pub struct Downloader {
    join_handle: Option<JoinHandle<()>>,
    handle: DownloaderHandle,
}

impl Downloader {
    pub fn new(store: Arc<RwLock<StoreManager>>, ep: Endpoint) -> Self {
        info!("Initializing downloader");

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DownloaderHandle { tx };

        // Initiate download tasks list
        let download_tasks: Vec<DownloadTask> = Vec::with_capacity(MAX_DOWNLOAD_TASKS);

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
            store,
            ep,
            download_tasks,
            handle.clone(),
        )));

        Downloader { join_handle, handle }
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        self.tx.send(DownloaderEvent::Shutdown).await.unwrap();
        self.join_handle.take().unwrap().await.unwrap();
    }

    /// Main method of the [`Downloader`].
    async fn handle_event(
        mut rx: mpsc::Receiver<DownloaderEvent>,
        store: Arc<RwLock<StoreManager>>,
        ep: Endpoint,
        mut download_tasks: Vec<DownloadTask>,
        downloader: DownloaderHandle,
    ) {
        // Process events
        while let Some(event) = rx.recv().await {
            match event {
                // Jobs
                DownloaderEvent::Accept(download_job) => {
                    // Event sent when a new download job is requested from the filesystem watcher
                    // If download tasks are available it will start downloading chunks.

                    store.write().await.add_download_job(download_job.clone()).await;

                    Self::spawn_download_tasks(
                        store.clone(),
                        ep.clone(),
                        download_job.clone(),
                        downloader.clone(),
                        &mut download_tasks,
                    ).await;
                }

                // Provision
                DownloaderEvent::RemoteProvisionUpdate(dir_uuid, node_id, file_hash, expiration) => {
                    // Event sent when a remote provision was updated for a file.
                    // (Received a message from the swarm of the availability of a file)
                    
                    if let Some(dir) = store.read().await.get_shared_dir(&dir_uuid) {
                        dir.write()
                            .await
                            .remote_provisions
                            .entry(file_hash)
                            .or_insert(HashMap::new())
                            .insert(node_id, expiration);
                    } else {
                        warn!("Received remote provision update for unknown UUID: {}", dir_uuid);
                    }
                    
                    if let Some(job) = store.read().await.get_next_download_job() {
                        Self::spawn_download_tasks(store.clone(), ep.clone(), job, downloader.clone(), &mut download_tasks).await;
                    }
                }

                DownloaderEvent::LocalProvisionUpdate(dir_uuid, file_hash, file_path) => {
                    // Event sent when a local provision was updated for a file.
                    // (Received a message from the swarm that requested the availability of a file)

                    let provision_dir = get_app_cache_dir().join("provisions");

                    if let Some(dir) = store.read().await.get_shared_dir(&dir_uuid) {
                        if !provision_dir.exists() {
                            if let Err(err) = tokio::fs::create_dir_all(&provision_dir).await {
                                error!("Cannot create cache directory: {err}");
                                return;
                            }
                        }

                        let mut encoder = {
                            match File::create(provision_dir.join(file_hash.to_string())) {
                                Ok(encode_file) => bao::encode::Encoder::new(encode_file),
                                Err(err) => {
                                    error!("Cannot create cache file: {err}");
                                    return;
                                }
                            }
                        };

                        if let Ok(file) = File::open(file_path.clone()) {
                            let mut reader = BufReader::new(file);
                            let mut buf = [0u8; CHUNK_SIZE];
                            while let Ok(len) = reader.read(&mut buf) {
                                if len == 0 {
                                    break;
                                }
                                if let Err(err) = encoder.write(&buf[0..len]) {
                                    error!("Cannot write to cache file: {err}");
                                }
                            }

                            match encoder.finalize() {
                                Ok(hash) => {
                                    if hash == file_hash {
                                        let expiration = Utc::now().add(PROVISION_EXPIRATION);
                                        dir.write().await.local_provisions.insert(hash, expiration);
                                        dir.handle()
                                            .await
                                            .send(ManagerEvent::ConfirmLocalProvision(hash, expiration))
                                            .await;
                                    } else {
                                        error!("Error while encoding file: {}", file_path.display());
                                    }
                                }
                                Err(err) => {
                                    error!("Error while finalizing file: {err}");
                                }
                            }
                        }
                    } else {
                        warn!("Received local provision update for unknown UUID: {}", dir_uuid);
                    }
                }

                DownloaderEvent::Supply {
                    conn,
                    uuid,
                    file_hash,
                    chunk_index,
                } => {
                    let provision_dir = get_app_cache_dir().join("provisions");

                    if let Some(dir) = store.read().await.get_shared_dir(&uuid) {
                        if let Ok(file) = File::open(provision_dir.join(file_hash.to_string())) {
                            let mut extractor = bao::encode::SliceExtractor::new(
                                file,
                                CHUNK_SIZE as u64 * chunk_index,
                                CHUNK_SIZE as u64,
                            );
                            let mut chunk = Vec::new();
                            if let Err(err) = extractor.read_to_end(&mut chunk) {
                                error!("Unable to get file slice: {}", err.to_string());
                                return;
                            }

                            let packet = SyncifyPacket::Blobs(BlobsPacket::Blob { chunk });

                            if let Err(err) = conn.clone().send_packet(dir, packet).await {
                                error!("Unable to send blob to {}: {err}", conn.remote());
                            }
                        }
                    }
                }

                DownloaderEvent::TaskFailed(download_job, chunk_index) => {
                    let mut job = download_job.write().await;

                    job.failed_chunks.push(chunk_index);

                    if job.chunk_done < *job.size() {
                        Self::spawn_download_tasks(store.clone(), ep.clone(), download_job.clone(), downloader.clone(), &mut download_tasks).await;
                    } else if !job.failed_chunks.is_empty() {
                        error!("Not implemented!");
                        // TODO: Handle failed chunks
                    } else {
                        job.set_state(JobState::Done(Utc::now()));
                        // TODO: Goto next download job
                    }
                }

                DownloaderEvent::TaskSuccess(download_job, chunk_index, decoded_data) => {
                    // TODO: What should we do when the cache file cannot be opened, seeked, written to ? Because it shouldn't happen...

                    let mut job = download_job.write().await;

                    // Handling the decoded data & cache file
                    let cache_file = get_app_cache_dir().join("downloads").join(job.hash().to_string());
                    let mut file = {
                        if cache_file.exists() {
                            if let Ok(file) = File::options().append(true).open(cache_file.clone()) {
                                file
                            } else {
                                error!("Cannot open cache file: {}", cache_file.display().to_string());
                                return;
                            }
                        } else if let Ok(file) = File::create(cache_file.clone()) {
                                file
                        } else {
                            error!("Cannot create cache file: {}", cache_file.display().to_string());
                            return;
                        }
                    };

                    if let Err(err) = file.seek(SeekFrom::Start(chunk_index * CHUNK_SIZE as u64)) {
                        error!("Cannot seek into the cache file: {err}");
                        return;
                    }
                    if let Err(err) = file.write_all(&decoded_data) {
                        error!("Cannot write to the cache file: {err}");
                        return;
                    }

                    // Handling the job update
                    job.progress = (job.chunk_done / job.size()) as f32;

                    // Launch new download tasks
                    if job.chunk_done < *job.size() {
                        Self::spawn_download_tasks(store.clone(), ep.clone(), download_job.clone(), downloader.clone(), &mut download_tasks).await;
                    } else if !job.failed_chunks.is_empty() {
                        error!("Not implemented!");
                        // TODO: Handle failed chunks
                    } else {
                        job.set_state(JobState::Done(Utc::now()));
                        // TODO: Goto next download job
                    }
                }

                // Actor
                DownloaderEvent::Shutdown => {
                    rx.close();
                    debug!("Closing event for the downloader")
                }
            }
        }

        info!("Finished event processing for the downloader");
    }

    /// Spawns as many download tasks as available in `download_tasks`
    ///
    /// Returns `true` if at least one task has been launched, otherwise returns `false`
    async fn spawn_download_tasks(
        store: Arc<RwLock<StoreManager>>,
        endpoint: Endpoint,
        download_job: Arc<RwLock<DownloadJob>>,
        download_handle: DownloaderHandle,
        download_tasks: &mut [DownloadTask],
    ) -> bool {
        info!("11");

        let mut ret = false;
        let mut chunk_index = 0;
        let mut job = download_job.write().await;

        info!("10");

        if *job.state() == JobState::Pending {
            job.set_state(JobState::Ongoing);
        } else {
            chunk_index = job.chunk_done;
        }

        if chunk_index < *job.size() {
            if let Some(dir) = store.read().await.get_shared_dir(job.uuid()) {
                info!("2");
                if let Some(node_list) = dir.read().await.remote_provisions.get(job.hash()) {
                    info!("3");
                    for node in node_list {
                        if Utc::now() < *node.1 {
                            info!("4");
                            for task in download_tasks.iter_mut() {
                                if task.handle.is_finished() {
                                    info!("5");
                                    task.handle = Self::spawn_download_task(
                                        *node.0,
                                        endpoint.clone(),
                                        download_job.clone(),
                                        chunk_index,
                                        dir.clone(),
                                        download_handle.clone(),
                                    );
                                    chunk_index += 1;
                                    ret = true;
                                }
                            }
                        }
                    }
                } else {
                    dir.handle().await.send(ManagerEvent::RequestProvision(*job.hash())).await;
                }
            }
        }

        info!("6");

        job.chunk_done = chunk_index;

        ret
    }

    fn spawn_download_task(
        node_id: NodeId,
        endpoint: Endpoint,
        download_job: Arc<RwLock<DownloadJob>>,
        chunk_index: u64,
        dir: SharedDirectory,
        download_handle: DownloaderHandle,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("8");
            let file_hash = *download_job.read().await.hash();
            if let Ok(mut connection) = SyncifyConnection::connect(node_id, endpoint).await {
                info!("7");
                let packet = SyncifyPacket::Blobs(BlobsPacket::BlobRequest {
                    file_hash: *file_hash.as_bytes(),
                    chunk_index,
                });
                info!("Sending blob request");
                if (connection.send_packet(dir.clone(), packet).await).is_err() {
                    download_handle
                        .send(DownloaderEvent::TaskFailed(download_job, chunk_index))
                        .await;
                    return;
                }
                info!("Wait for Blob");
                match connection.receive_packet(dir).await {
                    Ok(packet) => {
                        match packet {
                            SyncifyPacket::Blobs(BlobsPacket::Blob { chunk }) => {
                                info!("Blob received");
                                let mut decoded = Vec::new();
                                let mut decoder = bao::decode::SliceDecoder::new(
                                    &*chunk,
                                    &file_hash,
                                    CHUNK_SIZE as u64 * chunk_index,
                                    CHUNK_SIZE as u64,
                                );

                                if decoder.read_to_end(&mut decoded).is_err() {
                                    download_handle
                                        .send(DownloaderEvent::TaskFailed(download_job, chunk_index))
                                        .await;
                                } else {
                                    download_handle
                                        .send(DownloaderEvent::TaskSuccess(download_job, chunk_index, decoded))
                                        .await;
                                }
                            }
                            _ => {} // Will never happen
                        }
                    }
                    Err(_) => {
                        download_handle
                            .send(DownloaderEvent::TaskFailed(download_job, chunk_index))
                            .await;
                    }
                }
            }
        })
    }

    fn garbage_collect() {}
}

impl Deref for Downloader {
    type Target = DownloaderHandle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Handle to a [`Downloader`].
#[derive(Clone)]
pub struct DownloaderHandle {
    tx: mpsc::Sender<DownloaderEvent>,
}

impl DownloaderHandle {
    /// Send an [`DownloaderEvent`] to the actor.
    pub async fn send(&self, event: DownloaderEvent) {
        if let Err(e) = self.tx.send(event).await {
            error!("Error sending to downloader: {e}");
        }
    }

    /// Check if the actor is still alive.
    pub fn is_alive(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Event to control the [`Downloader`].
pub enum DownloaderEvent {
    // Jobs
    Accept(Arc<RwLock<DownloadJob>>),

    // Provision
    Supply {
        conn: SyncifyConnection,
        uuid: Uuid,
        file_hash: Hash,
        chunk_index: u64,
    },
    RemoteProvisionUpdate(Uuid, NodeId, Hash, DateTime<Utc>),
    LocalProvisionUpdate(Uuid, Hash, PathBuf),

    // Download task related
    TaskFailed(Arc<RwLock<DownloadJob>>, u64),
    TaskSuccess(Arc<RwLock<DownloadJob>>, u64, Vec<u8>),

    // Actor
    Shutdown,
}
