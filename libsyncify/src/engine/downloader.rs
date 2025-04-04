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
use crate::engine::job::{DownloadJob, JobState, LocalProvision, RemoteProvision};
use crate::engine::manager::ManagerEvent;
use crate::engine::protocol::{BlobsPacket, SyncifyPacket, SyncifyProtocol, SyncifyStream};
use crate::engine::state::Mutation;
use crate::store::StoreManager;
use crate::{SharedDirectory, get_app_cache_dir};
use blake3::Hash;
use chrono::Utc;
use iroh_base::NodeId;
use log::{debug, error, info, warn};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Size of the event buffer for [`Downloader`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Size of the chunk of file that are sent per packet.
pub const CHUNK_SIZE: usize = 64 * 1024;
/// Number of concurrent download threads
pub const MAX_DOWNLOAD_TASKS: usize = 10;

pub struct DownloadTask {
    handle: Option<JoinHandle<()>>,
}

pub struct Downloader {
    join_handle: Option<JoinHandle<()>>,
    handle: DownloaderHandle,
}

impl Downloader {
    pub fn new(store: Arc<RwLock<StoreManager>>, proto: Arc<RwLock<SyncifyProtocol>>) -> Self {
        info!("Initializing downloader");

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DownloaderHandle { tx };

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(rx, store, handle.clone(), proto)));

        Downloader { join_handle, handle }
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        let _ = self.tx.send(DownloaderEvent::Shutdown).await;
        let _ = self.join_handle.take().unwrap().await;
    }

    /// Main method of the [`Downloader`].
    async fn handle_event(
        mut rx: mpsc::Receiver<DownloaderEvent>,
        store: Arc<RwLock<StoreManager>>,
        downloader: DownloaderHandle,
        proto: Arc<RwLock<SyncifyProtocol>>,
    ) {
        // Creating the provision directory
        let provisions_dir = get_app_cache_dir().join("provisions");

        match provisions_dir.try_exists() {
            Ok(exists) => {
                if !exists {
                    if let Err(err) = tokio::fs::create_dir_all(&provisions_dir).await {
                        error!("Cannot create provisions directory ({err})");
                        return;
                    }
                }
            }
            Err(e) => {
                error!("Cannot check if provisions directory exists ({e})")
            }
        }

        // Creating the downloads directory
        let downloads_dir = get_app_cache_dir().join("downloads");

        match downloads_dir.try_exists() {
            Ok(exists) => {
                if !exists {
                    if let Err(err) = tokio::fs::create_dir_all(&downloads_dir).await {
                        error!("Cannot create downloads directory: {err}");
                        return;
                    }
                }
            }
            Err(e) => {
                error!("Cannot check if downloads directory exists ({e})")
            }
        }

        // Initiate download tasks list
        let mut download_tasks: Vec<DownloadTask> = Vec::with_capacity(MAX_DOWNLOAD_TASKS);
        for _ in 0..MAX_DOWNLOAD_TASKS {
            download_tasks.push(DownloadTask { handle: None });
        }

        /*// Resume unfinished downloads
        let jobs = store.read().await.get_download_jobs();
        for job in jobs {
            Self::spawn_download_tasks(
                store.clone(),
                downloads_dir.clone(),
                job,
                downloader.clone(),
                &mut download_tasks,
                proto.clone(),
            )
            .await;
        }*/

        // Process events
        while let Some(event) = rx.recv().await {
            match event {
                // Jobs
                DownloaderEvent::Accept(download_job) => {
                    // Event sent when a new download job is requested from the filesystem watcher
                    // If download tasks are available it will start downloading chunks.

                    // Verifying that the file from the job is not already downloaded
                    let job = download_job.read().await;
                    let dir_opt = store.read().await.get_shared_dir(job.dir_uuid());

                    if let Some(dir) = dir_opt {
                        let final_path = dir.path.join(match job.mutation() {
                            Mutation::Modify { file_path, .. } => file_path,
                            _ => {
                                unreachable!();
                            }
                        });

                        if let Some(hash_tree) = dir.local_tree.read().await.get(final_path.to_str().unwrap()) {
                            if hash_tree.hash() == *job.hash() {
                                debug!("File '{}' from {} has already been downloaded", *job.hash(), dir.uuid);
                                continue;
                            }
                        }
                    }
                    drop(job);

                    // Adding the job and starting downloading it
                    store.write().await.add_download_job(download_job.clone()).await;

                    Self::spawn_download_tasks(
                        store.clone(),
                        downloads_dir.clone(),
                        download_job.clone(),
                        downloader.clone(),
                        &mut download_tasks,
                        proto.clone(),
                    )
                    .await;
                }

                // Provision
                DownloaderEvent::RemoteProvisionUpdate(dir_uuid, provision) => {
                    // Event sent when a remote provision was updated for a file.
                    // (Received a message from the swarm of the availability of a file)

                    let dir_opt = store.read().await.get_shared_dir(&dir_uuid);
                    if let Some(dir) = dir_opt {
                        if let Err(e) = dir.remote_provisions.write().insert(provision.clone()).await {
                            warn!("Unable update provision: {e}")
                        }
                    } else {
                        warn!("Received remote provision update for unknown UUID: {}", dir_uuid);
                    }

                    let job_opt = store.read().await.get_download_job(provision.hash()).await;
                    if let Some(job) = job_opt {
                        Self::spawn_download_tasks(
                            store.clone(),
                            downloads_dir.clone(),
                            job,
                            downloader.clone(),
                            &mut download_tasks,
                            proto.clone(),
                        )
                        .await;
                    }
                }

                DownloaderEvent::LocalProvisionUpdate(dir_uuid, provision) => {
                    // Event sent when a local provision was updated for a file.
                    // (Received a message from the swarm that requested the availability of a file)
                    debug!("Updating local provision for {dir_uuid}");

                    let dir_opt = store.read().await.get_shared_dir(&dir_uuid);
                    if let Some(dir) = dir_opt {
                        let mut encoder = {
                            match File::options()
                                .read(true)
                                .write(true)
                                .create(true)
                                .open(provisions_dir.join(provision.hash().to_string()))
                            {
                                Ok(encode_file) => bao::encode::Encoder::new(encode_file),
                                Err(err) => {
                                    error!("Cannot create cache file ({err})");
                                    continue;
                                }
                            }
                        };

                        if let Ok(file) = File::open(provision.path()) {
                            let mut reader = BufReader::new(file);
                            if let Err(e) = std::io::copy(&mut reader, &mut encoder) {
                                error!("Cannot write cache file: {e}");
                                continue;
                            }

                            match encoder.finalize() {
                                Ok(hash) => {
                                    if hash == provision.hash() {
                                        if let Err(e) = dir.local_provisions.write().insert(provision.clone()).await {
                                            error!("Unable to insert provision: {e}");
                                        } else {
                                            debug!("Finished cutting the file");
                                            dir.handle()
                                                .await
                                                .send(ManagerEvent::ConfirmLocalProvision(provision.clone()))
                                                .await;
                                        }
                                    } else {
                                        error!("Error while encoding file: {}", provision.path().display());
                                        error!("{hash} {}", provision.hash())
                                    }
                                }
                                Err(err) => {
                                    error!("Error while finalizing file: {err}");
                                }
                            }
                        }
                    } else {
                        warn!("Received local provision update for unknown directory: {}", dir_uuid);
                    }
                }

                DownloaderEvent::Supply {
                    mut conn,
                    file_hash,
                    chunk_index,
                } => {
                    let dir = provisions_dir.clone();
                    tokio::spawn(async move {
                        if let Ok(file) = File::open(dir.join(file_hash.to_string())) {
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

                            if let Err(err) = conn.send_plain(&packet).await {
                                error!("Unable to send blob: {err}");
                            }
                        }
                    });
                }

                DownloaderEvent::TaskFailed(download_job, chunk_index) => {
                    Self::flush_download_tasks(&mut download_tasks);
                    let mut job = download_job.write().await;

                    job.failed_chunks.push(chunk_index);

                    if job.last_chunk < *job.size() {
                        drop(job);
                        Self::spawn_download_tasks(
                            store.clone(),
                            downloads_dir.clone(),
                            download_job.clone(),
                            downloader.clone(),
                            &mut download_tasks,
                            proto.clone(),
                        )
                        .await;
                    } else if !job.failed_chunks.is_empty() {
                        error!("Not implemented!");
                        // TODO: Handle failed chunks
                    }
                }

                DownloaderEvent::TaskSuccess(download_job, chunk_index, decoded_data) => {
                    // TODO: What should we do when the cache file cannot be opened, seeked, written to ? It shouldn't happen...
                    debug!("{}", chunk_index);
                    Self::flush_download_tasks(&mut download_tasks);
                    let download_job_tmp = download_job.clone();
                    let mut job = download_job.write().await;

                    let file: &mut BufWriter<File> = if let Some(buf) = &mut job.file {
                        buf
                    } else {
                        error!("Cache file cannot be found!");
                        continue;
                    };

                    if let Err(err) = file.seek(SeekFrom::Start(chunk_index * CHUNK_SIZE as u64)) {
                        error!("Cannot seek into the cache file: {err}");
                        continue;
                    }
                    if let Err(err) = file.write_all(&decoded_data) {
                        error!("Cannot write to the cache file: {err}");
                        continue;
                    }

                    // Handling the job update
                    job.chunk_done += 1;
                    job.progress = job.chunk_done as f32 / *job.size() as f32;
                    info!("Downloading... {:.1}%", job.progress * 100.0);

                    // Launch new download tasks
                    if job.chunk_done < *job.size() {
                        drop(job);
                        Self::spawn_download_tasks(
                            store.clone(),
                            downloads_dir.clone(),
                            download_job.clone(),
                            downloader.clone(),
                            &mut download_tasks,
                            proto.clone(),
                        )
                        .await;
                    } else if !job.failed_chunks.is_empty() {
                        drop(job);
                        error!("Not implemented!");
                        // TODO: Handle failed chunks
                    } else {
                        job.set_state(JobState::Done(Utc::now()));
                        info!("Received file {}", job.hash());

                        let file: &mut BufWriter<File> = if let Some(buf) = &mut job.file {
                            buf
                        } else {
                            error!("Cache file cannot be found!");
                            continue;
                        };

                        if let Err(e) = file.flush() {
                            error!("Unable to flush cache file: {e}");
                        }

                        job.file = None;

                        let dir_opt = store.read().await.get_shared_dir(&job.dir_uuid());
                        if let Some(dir) = dir_opt {
                            dir.handle()
                                .await
                                .send(ManagerEvent::DownloadFinished(download_job_tmp))
                                .await;
                        } else {
                            warn!(
                                "Received local provision update for unknown directory: {}",
                                job.dir_uuid()
                            );
                        }

                        drop(job);
                        store.write().await.flush_download_jobs().await;

                        let jobs = store.read().await.get_download_jobs();
                        for job in jobs {
                            Self::spawn_download_tasks(
                                store.clone(),
                                downloads_dir.clone(),
                                job,
                                downloader.clone(),
                                &mut download_tasks,
                                proto.clone(),
                            )
                            .await;
                        }
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
        download_dir: PathBuf,
        download_job: Arc<RwLock<DownloadJob>>,
        download_handle: DownloaderHandle,
        download_tasks: &mut [DownloadTask],
        proto: Arc<RwLock<SyncifyProtocol>>,
    ) {
        let mut job = download_job.write().await;

        if matches!(*job.state(), JobState::Pending) {
            match File::create(download_dir.join(job.hash().to_string())) {
                Ok(file) => job.file = Some(BufWriter::with_capacity(CHUNK_SIZE * 32, file)),
                Err(e) => {
                    error!("Cannot create cache file ({e})");
                }
            }
            job.set_state(JobState::Ongoing)
        } else if matches!(*job.state(), JobState::Ongoing) {
            match File::options()
                .create(true)
                .write(true)
                .truncate(false)
                .open(download_dir.join(job.hash().to_string()))
            {
                Ok(file) => job.file = Some(BufWriter::with_capacity(CHUNK_SIZE * 32, file)),
                Err(e) => {
                    error!("Cannot create cache file ({e})");
                }
            }
        }

        let dir_opt = store.read().await.get_shared_dir(job.dir_uuid());
        if let Some(dir) = dir_opt {
            let node_list_opt = dir.remote_provisions.read().await;
            if let Some(node_list) = node_list_opt.get(job.hash()) {
                for task in download_tasks.iter_mut() {
                    if job.last_chunk < *job.size() && task.handle.is_none() {
                        let nodes: Vec<(&NodeId, &RemoteProvision)> = node_list.iter().collect();
                        let node = nodes.get(job.last_chunk as usize % nodes.len()).unwrap();
                        if !node.1.is_expired() {
                            task.handle = Some(Self::spawn_download_task(
                                *node.0,
                                download_job.clone(),
                                job.last_chunk,
                                dir.clone(),
                                download_handle.clone(),
                                proto.clone(),
                            ));
                            job.last_chunk += 1;
                        }
                    }
                }
            } else {
                dir.handle()
                    .await
                    .send(ManagerEvent::RequestProvision(*job.hash()))
                    .await;
            }
        }
    }

    fn spawn_download_task(
        node_id: NodeId,
        download_job: Arc<RwLock<DownloadJob>>,
        chunk_index: u64,
        dir: SharedDirectory,
        download_handle: DownloaderHandle,
        proto: Arc<RwLock<SyncifyProtocol>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let file_hash = *download_job.read().await.hash();
            debug!("ACQUIRING PROTO");
            let mut proto = proto.write().await;
            if let Ok(mut connection) = proto.open_stream(&dir, node_id).await {
                drop(proto);
                debug!("RELEASING PROTO");
                let packet = SyncifyPacket::Blobs(BlobsPacket::BlobRequest {
                    file_hash: *file_hash.as_bytes(),
                    chunk_index,
                });
                if (connection.send(&packet).await).is_err() {
                    download_handle
                        .send(DownloaderEvent::TaskFailed(download_job, chunk_index))
                        .await;
                    return;
                }
                match connection.recv().await {
                    Ok(packet) => {
                        match packet {
                            SyncifyPacket::Blobs(BlobsPacket::Blob { chunk }) => {
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
                    Err(err) => {
                        debug!("{err}");
                        download_handle
                            .send(DownloaderEvent::TaskFailed(download_job, chunk_index))
                            .await;
                    }
                }
            }
            debug!("RELEASING PROTO");
        })
    }

    fn flush_download_tasks(download_tasks: &mut Vec<DownloadTask>) {
        for task in download_tasks {
            if let Some(t) = &task.handle {
                if t.is_finished() {
                    task.handle = None;
                }
            }
        }
    }

    fn _garbage_collect() {}
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
        conn: SyncifyStream,
        file_hash: Hash,
        chunk_index: u64,
    },
    RemoteProvisionUpdate(Uuid, RemoteProvision),
    LocalProvisionUpdate(Uuid, LocalProvision),

    // Download task related
    TaskFailed(Arc<RwLock<DownloadJob>>, u64),
    TaskSuccess(Arc<RwLock<DownloadJob>>, u64, Vec<u8>),

    // Actor
    Shutdown,
}
