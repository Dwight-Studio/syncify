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
use std::collections::HashMap;
use std::sync::Arc;
use blake3::Hash;
use iroh_blobs::net_protocol::Blobs;
use log::{debug, error, info, warn};
use std::ops::Deref;
use chrono::{DateTime, TimeDelta, Utc};
use iroh_base::NodeId;
use rkyv::{Archive, Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;
use crate::engine::job::Job;

pub const EVENT_BUFFER_SIZE: usize = 1024;
pub const CHUNK_SIZE: usize = 16 * 1024;

#[derive(Archive, Serialize, Deserialize)]
pub(crate) struct Provided {
    node: [u8; 32],
    #[rkyv(with = crate::util::DateTimeDef)]
    expire: DateTime<Utc>,
}

impl Provided {
    /// Check expiration.
    ///
    /// # Return
    ///
    /// Returns true if expired, false otherwise.
    pub fn expired(&self) -> bool {
        self.expire.signed_duration_since(Utc::now()).le(&TimeDelta::zero())
    }
}

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
        let files_they_provide: HashMap<Uuid, HashMap<[u8; 32], Vec<Provided>>> = HashMap::new();
        let files_i_provide: HashMap<Hash, DateTime<Utc>> = HashMap::new();

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(rx, files_they_provide, files_i_provide)));

        Downloader { join_handle, handle }
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        self.tx.send(DownloaderEvent::Shutdown).await.unwrap();
        self.join_handle.take().unwrap().await.unwrap();
    }

    /// Main method of the [`Downloader`].
    async fn handle_event(mut rx: mpsc::Receiver<DownloaderEvent>, mut files_they_provide: HashMap<Uuid, HashMap<[u8; 32], Vec<Provided>>>, mut files_i_provide: HashMap<Hash, DateTime<Utc>>) {
        while let Some(event) = rx.recv().await {
            match event {
                // Jobs
                DownloaderEvent::Accept(_) => {}
                
                // Provision
                DownloaderEvent::Provide(uuid, node_id, file_hash, expire) => {
                    let file_map = &mut *files_they_provide.get_mut(&uuid).unwrap();
                    file_map.entry(*file_hash.as_bytes()).or_default().push(Provided {
                        node: *node_id.as_bytes(),
                        expire,
                    });
                }
                
                DownloaderEvent::Provision(file_hash, expire) => {
                    
                }
                
                DownloaderEvent::Supply {
                    uuid,
                    file_hash,
                    from,
                    to,
                } => {}
                
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
    // Jobs
    Accept(Arc<RwLock<Job>>),
    
    // Provision
    Supply {
        uuid: Uuid,
        file_hash: Hash,
        from: u64,
        to: u64,
    },
    Provide(Uuid, NodeId, Hash, DateTime<Utc>),
    Provision(Hash, DateTime<Utc>),
    

    // Actor
    Shutdown,
}
