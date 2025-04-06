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
use crate::engine::downloader::worker::{DownloadChunk, DownloadWorker};
use crate::engine::downloader::writer::{DownloadWriter, DownloadedChunk, WRITER_BUFFER_LENGTH};
use crate::engine::job::{DownloadJob, JobState, LocalProvision, RemoteProvision};
use crate::engine::manager::ManagerEvent;
use crate::engine::protocol::{BlobsPacket, SyncifyPacket, SyncifyProtocol, SyncifyStream};
use crate::event::{DownloadEvent, EngineEvent, EventSender};
use crate::store::StoreManager;
use crate::store::lock::StoreLock;
use crate::{SharedDirectory, get_app_cache_dir};
use blake3::Hash;
use log::{debug, error, info, warn};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use uuid::Uuid;

mod worker;
pub mod writer;

/// Size of the [`DownloaderEvent`] buffer for [`Downloader`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Size of the chunk of file that are sent per packet.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// Number of concurrent download threads
pub const MAX_DOWNLOAD_WORKER: usize = 10;

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

        // Initiate download tasks list
        let mut file_writer_map: HashMap<Hash, Sender<DownloadedChunk>> = HashMap::new();
        let mut download_tasks: Vec<JoinHandle<()>> = Vec::with_capacity(MAX_DOWNLOAD_WORKER);
        let cancel_list: Arc<RwLock<Vec<Hash>>> = Arc::new(RwLock::new(Vec::new()));

        let (workers, recv) = async_channel::unbounded();
        for _ in 0..MAX_DOWNLOAD_WORKER {
            let mut worker = DownloadWorker {
                rx: recv.clone(),
                cancel_list: cancel_list.clone(),
                proto: proto.clone(),
            };
            download_tasks.push(tokio::spawn(async move { worker.run().await }));
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
                        Self::start_download(&sender, &mut file_writer_map, workers.clone(), dir, job).await;
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
                    /*Self::download_next(
                        &store,
                        &sender,
                        &downloads_dir,
                        &jobs,
                        &downloader,
                        &mut download_tasks,
                        &proto,
                    )
                    .await;*/
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
                            if matches!(job.read().await.state, JobState::Pending) {
                                Self::start_download(&sender, &mut file_writer_map, workers.clone(), dir, job.clone())
                                    .await;
                            }
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

    async fn start_download(
        sender: &EventSender,
        file_writer_map: &mut HashMap<Hash, Sender<DownloadedChunk>>,
        workers: async_channel::Sender<DownloadChunk>,
        dir: SharedDirectory,
        job: StoreLock<DownloadJob>,
    ) {
        let job_opt = job.read().await;

        if let Some(mut prov) = dir.remote_provisions.read().await.get(job_opt.hash()).cloned() {
            prov.retain(|_, provision| !provision.is_expired());
            if !prov.is_empty() {
                let (writer_tx, writer_rx) = mpsc::channel(WRITER_BUFFER_LENGTH);
                let mut download_writer = DownloadWriter::new(writer_rx, job.clone(), sender.clone(), dir.clone());

                tokio::spawn(async move { download_writer.run().await });
                file_writer_map.insert(*job_opt.hash(), writer_tx.clone());

                for chunk_index in 0..*job_opt.size() {
                    if let Err(err) = workers
                        .send(DownloadChunk {
                            index: chunk_index,
                            hash: *job_opt.hash(),
                            writer_tx: writer_tx.clone(),
                            dir: dir.clone(),
                        })
                        .await
                    {
                        error!("Cannot send message to worker ({err})");
                    }
                }
            } else {
                dir.handle()
                    .await
                    .send(ManagerEvent::RequestProvision(*job_opt.hash()))
                    .await;
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

    // Actor
    Shutdown,
}
