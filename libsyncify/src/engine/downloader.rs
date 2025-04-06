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
use crate::event::{DownloadEvent, EngineEvent, EventSender};
use crate::store::StoreManager;
use crate::store::lock::StoreLock;
use crate::{SharedDirectory, get_app_cache_dir};
use blake3::Hash;
use iroh_base::NodeId;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Size of the [`DownloaderEvent`] buffer for [`Downloader`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Size of the chunk of file that are sent per packet.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// Number of concurrent download threads
pub const MAX_DOWNLOAD_TASKS: usize = 10;

pub struct DownloadTask {
    handle: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub struct Downloader {
    join_handle: Option<JoinHandle<()>>,
    handle: DownloaderHandle,
}

impl Downloader {
    pub async fn new(store: Arc<RwLock<StoreManager>>, sender: EventSender, mut proto: SyncifyProtocol) -> Self {
        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DownloaderHandle { tx };

        // Add handle to protocol
        proto.set_downloader(handle.clone());

        // Load jobs
        let jobs = StoreManager::load_jobs(&store).await.unwrap_or_else(|e| {
            error!("Cannot load jobs ({e})");
            HashMap::new()
        });

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
            store,
            sender,
            jobs,
            handle.clone(),
            proto,
        )));

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
        sender: EventSender,
        mut jobs: HashMap<Hash, StoreLock<DownloadJob>>,
        downloader: DownloaderHandle,
        proto: SyncifyProtocol,
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

        // Notify
        info!("Started downloader");
        sender.send(EngineEvent::DownloaderStarted.wrap());

        // Process events
        while let Some(event) = rx.recv().await {
            match event {
                // Jobs
                DownloaderEvent::Accept(job) => {
                    // Event sent when a new download job is requested from the filesystem watcher
                    // If download tasks are available it will start downloading chunks.

                    let file_hash = *job.hash();
                    let dir_uuid = *job.dir_uuid();

                    // Verifying that the file from the job is not already downloaded
                    let dir_opt = store.read().await.get_shared_dir(&dir_uuid);

                    if let Some(dir) = dir_opt {
                        if let Some(hash_tree) = dir.local_tree.read().await.get(job.path()) {
                            if hash_tree.hash() == *job.hash() {
                                debug!("File '{}' from {} has already been downloaded", *job.hash(), dir.uuid);
                                continue;
                            }
                        }

                        // Add the job
                        let job = StoreLock::new(&store, job, ());
                        if let Err(e) = job.write().flush().await {
                            error!("Cannot flush job ({e})");
                        }

                        jobs.insert(file_hash, job.clone());

                        // Start downloading it
                        Self::spawn_download_tasks(
                            &sender,
                            &downloads_dir,
                            &job,
                            &file_hash,
                            dir,
                            &downloader,
                            &mut download_tasks,
                            &proto,
                        )
                        .await;
                    }
                }

                DownloaderEvent::Cancel(hash) => {
                    if let Some(job) = jobs.remove(&hash) {
                        let j = job.read().await;

                        info!("Cancelling download of '{hash}'");
                        sender.send(
                            DownloadEvent::DownloadCompleted {
                                dir_uuid: j.dir_uuid,
                                file_hash: *j.hash(),
                            }
                            .wrap(),
                        );
                        drop(j);

                        job.write().cancel().await;
                    }
                }

                DownloaderEvent::Resume => {
                    Self::download_next(
                        &store,
                        &sender,
                        &downloads_dir,
                        &jobs,
                        &downloader,
                        &mut download_tasks,
                        &proto,
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

                        sender.send(DownloadEvent::RemoteProvisionUpdate(provision.clone()).wrap());

                        if let Some(job) = jobs.get(provision.hash()) {
                            Self::spawn_download_tasks(
                                &sender,
                                &downloads_dir,
                                job,
                                provision.hash(),
                                dir,
                                &downloader,
                                &mut download_tasks,
                                &proto,
                            )
                            .await;
                        } else {
                            debug!("The provision doesn't match a pending download job")
                        }
                    } else {
                        warn!("Received remote provision update for unknown directory {}", dir_uuid);
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
                                            error!("Cannot insert provision: {e}");
                                        } else {
                                            debug!("Finished encoding the file");
                                            sender.send(DownloadEvent::LocalProvisionUpdate(provision.clone()).wrap());
                                            dir.handle()
                                                .await
                                                .send(ManagerEvent::ConfirmLocalProvision(provision))
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

                DownloaderEvent::SupplyBlob {
                    mut conn,
                    file_hash,
                    chunk_index,
                } => {
                    let provision_dir = provisions_dir.clone();
                    tokio::spawn(async move {
                        if let Ok(file) = File::open(provision_dir.join(file_hash.to_string())) {
                            let mut extractor = bao::encode::SliceExtractor::new(
                                file,
                                CHUNK_SIZE as u64 * chunk_index,
                                CHUNK_SIZE as u64,
                            );
                            let mut chunk = Vec::new();
                            if let Err(err) = extractor.read_to_end(&mut chunk) {
                                error!("Cannot get slice of '{file_hash}' ({})", err.to_string());
                                return;
                            }

                            let packet = SyncifyPacket::Blob(BlobsPacket::Blob { chunk });

                            if let Err(err) = conn.send_plain(&packet).await {
                                error!("Cannot supply blob '{file_hash}' ({err})");
                            }
                        } else {
                            debug!("Cannot supply blob '{file_hash}' (no provision)");
                        }
                    });
                }

                DownloaderEvent::TaskFailed(job, chunk_index, dir, _node_id) => {
                    Self::flush_download_tasks(&mut download_tasks);

                    if !matches!(job.read().await.state(), JobState::Cancelled) {
                        // Atomically load all variables
                        let (file_hash, is_done, empty_failed_chunks) = {
                            let j = job.read().await;

                            (*j.hash(), j.is_done(), j.failed_chunks.is_empty())
                        };

                        job.write().add_failed_chunk(chunk_index).await;

                        if !is_done {
                            Self::spawn_download_tasks(
                                &sender,
                                &downloads_dir,
                                &job,
                                &file_hash,
                                dir,
                                &downloader,
                                &mut download_tasks,
                                &proto,
                            )
                            .await;
                        } else if !empty_failed_chunks {
                            error!("Not implemented!");
                            // TODO: Handle failed chunks
                        }
                    } else {
                        Self::download_next(
                            &store,
                            &sender,
                            &downloads_dir,
                            &jobs,
                            &downloader,
                            &mut download_tasks,
                            &proto,
                        )
                        .await;
                    }
                }

                DownloaderEvent::TaskSuccess(job, chunk_index, decoded_data, dir, node_id) => {
                    // TODO: What should we do when the cache file cannot be opened, seeked, written to ? It shouldn't happen...
                    Self::flush_download_tasks(&mut download_tasks);

                    if !matches!(job.read().await.state(), JobState::Cancelled) {
                        if !job.write().finish_download_chunk(chunk_index, decoded_data).await {
                            continue;
                        }

                        // Atomically load all variables
                        let (file_hash, is_done, progress, empty_failed_chunks) = {
                            let j = job.read().await;

                            (*j.hash(), j.is_done(), j.progress(), j.failed_chunks.is_empty())
                        };

                        debug!("Downloading {file_hash}... {:.1}%", progress * 100.0);
                        sender.send(
                            DownloadEvent::DownloadProgressed {
                                dir_uuid: dir.uuid,
                                file_hash,
                                progress,
                                download_chunk_index: chunk_index,
                                peer_node_id: node_id,
                            }
                            .wrap(),
                        );

                        // Launch new download tasks
                        if !is_done {
                            Self::spawn_download_tasks(
                                &sender,
                                &downloads_dir,
                                &job,
                                &file_hash,
                                dir,
                                &downloader,
                                &mut download_tasks,
                                &proto,
                            )
                            .await;
                        } else if !empty_failed_chunks {
                            error!("Not implemented!");
                            // TODO: Handle failed chunks
                        } else {
                            info!("Received file {}", file_hash);
                            sender.send(
                                DownloadEvent::DownloadCompleted {
                                    dir_uuid: dir.uuid,
                                    file_hash,
                                }
                                .wrap(),
                            );

                            if !job.write().finish_download().await {
                                continue;
                            }

                            dir.handle()
                                .await
                                .send(ManagerEvent::DownloadFinished(job.clone()))
                                .await;

                            // Remove job from list
                            jobs.remove(&file_hash);

                            Self::download_next(
                                &store,
                                &sender,
                                &downloads_dir,
                                &jobs,
                                &downloader,
                                &mut download_tasks,
                                &proto,
                            )
                            .await;
                        }
                    } else {
                        Self::download_next(
                            &store,
                            &sender,
                            &downloads_dir,
                            &jobs,
                            &downloader,
                            &mut download_tasks,
                            &proto,
                        )
                        .await;
                    }
                }

                // Actor
                DownloaderEvent::Shutdown => {
                    rx.close();
                    debug!("Closing event for the downloader");
                    sender.send(EngineEvent::DownloaderStopped.wrap());
                }
            }
        }

        info!("Finished event processing for the downloader");
        sender.send(EngineEvent::DownloaderFinished.wrap());
    }

    /// Spawns as many download tasks as available in `download_tasks` for all pending [`DownloadJob`].
    async fn download_next(
        store: &Arc<RwLock<StoreManager>>,
        sender: &EventSender,
        downloads_dir: &Path,
        jobs: &HashMap<Hash, StoreLock<DownloadJob>>,
        download_handle: &DownloaderHandle,
        download_tasks: &mut [DownloadTask],
        proto: &SyncifyProtocol,
    ) {
        for (hash, job) in jobs {
            let uuid = job.read().await.dir_uuid;
            let dir_opt = store.read().await.get_shared_dir(&uuid);
            let dir = match dir_opt {
                Some(dir) => dir,
                None => {
                    error!("Unknown directory {uuid}");
                    if let Err(e) = job.write().delete().await {
                        error!("Cannot delete job for unknown directory {uuid} ({e})")
                    }
                    continue;
                }
            };
            Self::spawn_download_tasks(
                &sender,
                downloads_dir,
                job,
                hash,
                dir,
                download_handle,
                download_tasks,
                proto,
            )
            .await;
        }
    }

    /// Spawns as many download tasks as available in `download_tasks`.
    ///
    /// # Return
    ///
    /// Returns `true` if at least one task has been launched, otherwise returns `false`.
    async fn spawn_download_tasks(
        sender: &EventSender,
        downloads_dir: &Path,
        job: &StoreLock<DownloadJob>,
        file_hash: &Hash,
        dir: SharedDirectory,
        download_handle: &DownloaderHandle,
        download_tasks: &mut [DownloadTask],
        proto: &SyncifyProtocol,
    ) {
        if !job.write().start_download(sender, downloads_dir).await {
            return;
        }

        let node_list_opt = dir.remote_provisions.read().await;
        if let Some(node_list) = node_list_opt.get(file_hash) {
            for task in download_tasks.iter_mut() {
                // Atomically load all variables
                let last_chunk = {
                    let j = job.read().await;

                    // Stop the loop if all chunks are downloading
                    if j.all_chunks_downloading() {
                        break;
                    }

                    j.last_chunk
                };

                if task.handle.is_none() {
                    let nodes: Vec<(&NodeId, &RemoteProvision)> = node_list.iter().collect();
                    let node = nodes.get(last_chunk as usize % nodes.len()).unwrap();
                    if !node.1.is_expired() {
                        task.handle = Some(Self::spawn_download_task(
                            *node.0,
                            job.clone(),
                            last_chunk,
                            dir.clone(),
                            download_handle.clone(),
                            proto.clone(),
                        ));
                        job.write().start_download_chunk().await;
                    }
                }
            }
        } else {
            dir.handle()
                .await
                .send(ManagerEvent::RequestProvision(*file_hash))
                .await;
        }
    }

    /// Spawn a download task.
    fn spawn_download_task(
        node_id: NodeId,
        download_job: StoreLock<DownloadJob>,
        chunk_index: u64,
        dir: SharedDirectory,
        download_handle: DownloaderHandle,
        mut proto: SyncifyProtocol,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let file_hash = *download_job.read().await.hash();
            if let Ok(mut connection) = proto.open_stream(&dir, node_id).await {
                drop(proto);
                let packet = SyncifyPacket::Blob(BlobsPacket::BlobRequest { file_hash, chunk_index });
                if connection.send(&packet).await.is_err() {
                    download_handle
                        .send(DownloaderEvent::TaskFailed(
                            download_job.clone(),
                            chunk_index,
                            dir,
                            node_id,
                        ))
                        .await;
                    return;
                }
                match connection.recv().await {
                    Ok(packet) => {
                        match packet {
                            SyncifyPacket::Blob(BlobsPacket::Blob { chunk }) => {
                                let mut decoded = Vec::new();
                                let mut decoder = bao::decode::SliceDecoder::new(
                                    &*chunk,
                                    &file_hash,
                                    CHUNK_SIZE as u64 * chunk_index,
                                    CHUNK_SIZE as u64,
                                );

                                if decoder.read_to_end(&mut decoded).is_err() {
                                    download_handle
                                        .send(DownloaderEvent::TaskFailed(download_job, chunk_index, dir, node_id))
                                        .await;
                                } else {
                                    download_handle
                                        .send(DownloaderEvent::TaskSuccess(
                                            download_job,
                                            chunk_index,
                                            decoded,
                                            dir,
                                            node_id,
                                        ))
                                        .await;
                                }
                            }
                            _ => unreachable!(), // Will never happen
                        }
                    }
                    Err(err) => {
                        debug!("{err}");
                        download_handle
                            .send(DownloaderEvent::TaskFailed(download_job, chunk_index, dir, node_id))
                            .await;
                    }
                }
            }
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

    fn _garbage_collect() {
        // TODO: Delete expired provisions
        // TODO: Delete non expired provisions when the PROVISION_CACHE_MAX_SIZE (defined in gossip) is exceeded
    }
}

impl Deref for Downloader {
    type Target = DownloaderHandle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Handle to a [`Downloader`].
#[derive(Debug, Clone)]
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
    Accept(DownloadJob),
    Cancel(Hash),
    Resume,

    // Provision
    SupplyBlob {
        conn: SyncifyStream,
        file_hash: Hash,
        chunk_index: u64,
    },
    RemoteProvisionUpdate(Uuid, RemoteProvision),
    LocalProvisionUpdate(Uuid, LocalProvision),

    // Download task related
    TaskFailed(StoreLock<DownloadJob>, u64, SharedDirectory, NodeId),
    TaskSuccess(StoreLock<DownloadJob>, u64, Vec<u8>, SharedDirectory, NodeId),

    // Actor
    Shutdown,
}
