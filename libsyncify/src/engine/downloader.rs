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
use std::ops::Deref;
use blake3::Hash;
use iroh_blobs::net_protocol::Blobs;
use log::{debug, error, info};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use uuid::Uuid;

pub const EVENT_BUFFER_SIZE: usize = 1024;
pub const CHUNK_SIZE: usize = 16 * 1024;

pub struct Downloader {
    join_handle: Option<JoinHandle<()>>,
    handle: DownloaderHandle,
}

impl Downloader {
    pub fn new(blobs: Blobs<iroh_blobs::store::fs::Store>) -> Self {
        info!("Initializing downloader");

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DownloaderHandle { tx };

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
        )));

        Downloader {
            join_handle,
            handle
        }
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        self.tx.send(DownloaderEvent::Shutdown).await.unwrap();
        self.join_handle.take().unwrap().await.unwrap();
    }

    /// Main method of the [`Downloader`].
    async fn handle_event(
        mut rx: mpsc::Receiver<DownloaderEvent>,
    ) {
        while let Some(event) = rx.recv().await {
            match event {
                DownloaderEvent::Request { uuid, file_hash, from, to} => {
                    
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
    pub async fn send(&self, event: DownloaderEvent) {
        if let Err(e) = self.tx.send(event).await {
            error!("Error sending to downloader: {e}");
        }
    }
}

/// Event to control the [`Downloader`].
pub enum DownloaderEvent {
    Request{uuid: Uuid, file_hash: Hash, from: u64, to: u64},
    // Actor
    Shutdown,
}