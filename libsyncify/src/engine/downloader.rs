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
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::sync::Arc;
use blake3::Hash;
use log::{debug, error, info, warn};
use std::ops::Deref;
use std::path::PathBuf;
use chrono::{DateTime, TimeDelta, Utc};
use iroh_base::NodeId;
use redb::{Key, TypeName, Value};
use rkyv::{Archive, Deserialize, Serialize};
use rkyv::util::AlignedVec;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use uuid::Uuid;
use crate::engine::job::DownloadJob;
use crate::get_app_cache_dir;
use crate::store::StoreManager;

/// Size of the event buffer for [`Downloader`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Size of the chunk of file that are sent per packet.
pub const CHUNK_SIZE: usize = 16 * 1024;

#[derive(Archive, Serialize, Deserialize, Debug)]
pub(crate) struct Provision {
    node: [u8; 32],
    hash: [u8; 32],
    #[rkyv(with = crate::util::DateTimeDef)]
    expire: DateTime<Utc>,
}

impl Value for Provision {
    type SelfType<'a> = Provision;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        Option::from(size_of::<Provision>())
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        rkyv::from_bytes::<Provision, rkyv::rancor::Error>(data).unwrap_or_else(|e| {
            error!("Failed to deserialize download job: {e}");
            return Provision {
                node: [0u8; 32],
                hash: [0u8; 32],
                expire: Default::default(),
            }
        })
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'b,
    {
        rkyv::to_bytes(value).unwrap_or_else(|e: rkyv::rancor::Error| {
            error!("Failed to serialize download job: {e}");
            return AlignedVec::new();
        }).to_vec().leak()
    }

    fn type_name() -> TypeName {
        TypeName::new("Provided")
    }
}

impl Key for Provision {
    //noinspection RsTraitObligations
    fn compare(data1_bytes: &[u8], data2_bytes: &[u8]) -> Ordering {
        if let (Ok(data1), Ok(data2)) = (rkyv::from_bytes::<Provision, rkyv::rancor::Error>(data1_bytes), rkyv::from_bytes::<Provision, rkyv::rancor::Error>(data2_bytes)) {
            match data1.hash.cmp(&data2.hash) {
                Ordering::Equal => data1.node.cmp(&data2.node),
                other => other,
            }
        } else {
            Ordering::Greater
        }
    }
}

impl Provision {
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
    pub fn new(store: Arc<RwLock<StoreManager>>) -> Self {
        info!("Initializing downloader");

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DownloaderHandle { tx };

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
            store
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
        store: Arc<RwLock<StoreManager>>
    ) {
        // TODO: Load files_they_provide and files_i_provide from store (provisions)
        
        // Process events
        while let Some(event) = rx.recv().await {
            match event {
                // Jobs
                DownloaderEvent::Accept(download_job) => {
                    
                }
                
                // Provision
                DownloaderEvent::RemoteProvision(uuid, node_id, file_hash, expire) => {
                    let file_map = &mut *files_they_provide.get_mut(&uuid).unwrap();
                    file_map.entry(file_hash).or_default().push(Provision {
                        node: *node_id.as_bytes(),
                        hash: *file_hash.as_bytes(),
                        expire,
                    });
                }

                DownloaderEvent::LocalProvision(file_hash, file_path, expire) => {
                    if !get_app_cache_dir().exists() {
                        if let Err(err) = tokio::fs::create_dir_all(&get_app_cache_dir()).await {
                            error!("Cannot create cache directory: {err}");
                        }
                    }

                    let mut encoder = {
                        match File::create(get_app_cache_dir().join(file_hash.to_string())) {
                            Ok(encode_file) => { bao::encode::Encoder::new(encode_file) }
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
                            if let Err(err) = encoder.write(&buf[0..len]) {
                                error!("Cannot write to cache file: {err}");
                            }
                        }

                        if let Ok(hash) = encoder.finalize() {
                            if hash == file_hash {
                                files_i_provide.insert(hash, expire);
                            } else {
                                error!("Error while encoding file: {}", file_path.display());
                            }
                        }
                    }
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

    fn garbage_collect() {

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
        uuid: Uuid,
        file_hash: Hash,
        from: u64,
        to: u64,
    },
    RemoteProvision(Uuid, NodeId, Hash, DateTime<Utc>),
    LocalProvision(Hash, PathBuf, DateTime<Utc>),

    // Actor
    Shutdown,
}
