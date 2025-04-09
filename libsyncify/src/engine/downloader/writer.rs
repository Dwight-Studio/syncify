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
use crate::engine::downloader::{DownloaderEvent, DownloaderHandle, CHUNK_SIZE};
use crate::engine::job::{DownloadJob, JobState};
use crate::engine::manager::ManagerEvent;
use crate::event::{DownloadEvent, EventSender};
use crate::store::lock::StoreLock;
use crate::{SharedDirectory, get_app_cache_dir};
use iroh_base::NodeId;
use log::{error, info};
use std::io::{SeekFrom, Write};
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::sync::mpsc::Receiver;

/// Size of the [`WriterChunk`] buffer for [`DownloadWriter`].
pub const WRITER_BUFFER_LENGTH: usize = 1024;

pub struct DownloadedChunk {
    pub(crate) index: u64,
    pub(crate) data: Vec<u8>,
    pub(crate) node_id: NodeId,
}

pub struct FailedChunk {
    pub(crate) index: u64,
}

pub enum WriterChunk {
    DownloadedChunk(DownloadedChunk),
    FailedChunk(FailedChunk),
}

#[derive(Debug)]
pub struct DownloadWriter {
    pub(crate) rx: Receiver<WriterChunk>,
    pub(crate) job: StoreLock<DownloadJob>,
    pub(crate) event_sender: EventSender,
    pub(crate) dir: SharedDirectory,
    pub(crate) downloader: DownloaderHandle,
}

impl DownloadWriter {
    pub fn new(
        rx: Receiver<WriterChunk>,
        job: StoreLock<DownloadJob>,
        event_sender: EventSender,
        dir: SharedDirectory,
        downloader: DownloaderHandle,
    ) -> Self {
        Self {
            rx,
            job,
            event_sender,
            dir,
            downloader
        }
    }

    pub async fn run(&mut self) {
        // Creating the downloads directory
        let downloads_dir = get_app_cache_dir().join("downloads");
        let (file_hash, dir_uuid, file_size) = {
            let j = self.job.read().await;

            (*j.hash(), j.dir_uuid, *j.size())
        };

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

        let mut file = match File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(downloads_dir.join(file_hash.to_string()))
            .await
        {
            Ok(file) => {
                self.job.write().set_ongoing().await;
                self.event_sender
                    .send(DownloadEvent::DownloadStarted { dir_uuid, file_hash, file_size }.wrap());
                BufWriter::with_capacity(CHUNK_SIZE * 16, file)
            }
            Err(e) => {
                error!("Cannot create cache file ({e})");
                return;
            }
        };
        
        while let Some(rcv) = self.rx.recv().await {
            if matches!(self.job.read().await.state, JobState::Cancelled) {
                break;
            }
            
            match rcv {
                WriterChunk::DownloadedChunk(chunk) => {
                    if let Err(err) = file.seek(SeekFrom::Start(chunk.index * CHUNK_SIZE as u64)).await {
                        error!("Cannot seek into the cache file: {err}");
                    }
                    if let Err(err) = file.write_all(&chunk.data).await {
                        error!("Cannot write to the cache file: {err}");
                    }

                    let (progress, is_done, all_chunk_tried) = self.job.write().finish_download_chunk().await;
                    print!("Downloading {file_hash}... {:.1}%\r", progress * 100.0);
                    std::io::stdout().flush().unwrap();
                    self.event_sender.send(
                        DownloadEvent::DownloadProgressed {
                            dir_uuid,
                            file_hash,
                            progress,
                            download_chunk_index: chunk.index,
                            peer_node_id: chunk.node_id,
                        }
                            .wrap(),
                    );

                    if is_done {
                        info!("Received file {}", file_hash);
                        self.event_sender
                            .send(DownloadEvent::DownloadCompleted { dir_uuid, file_hash }.wrap());

                        if let Err(e) = file.flush().await {
                            error!("Cannot flush cache file: {e}");
                        }

                        self.job.write().finish_download().await;

                        self.dir
                            .handle()
                            .await
                            .send(ManagerEvent::DownloadFinished(self.job.clone()))
                            .await;

                        break;
                    } else if all_chunk_tried {
                        self.downloader.send(DownloaderEvent::Resume).await;
                        break;
                    }
                }
                WriterChunk::FailedChunk(chunk) => {
                    let all_chunk_tried = self.job.write().add_failed_chunk(chunk.index).await;
                    
                    if all_chunk_tried {
                        self.downloader.send(DownloaderEvent::Resume).await;
                        break;
                    }
                }
            }
        }
    }
}
