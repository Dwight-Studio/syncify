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
use crate::SharedDirectory;
use crate::engine::manager::{ManagerHandle, ManagerEvent, SyncEvent};
use crate::engine::protocol::SyncifyProtocol;
use crate::engine::state::HashTree;
use blake3::Hash;
use bytes::Bytes;
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, Key, KeyInit, XChaCha20Poly1305, XNonce};
use chrono::{DateTime, TimeDelta, Utc};
use iroh::{NodeAddr, NodeId};
use iroh_gossip::net::{GossipEvent, GossipSender};
use log::{info, warn};
use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;
use crate::engine::downloader::{DownloaderEvent, DownloaderHandle};

pub const PROVIDES_EXPIRATION_HOURS_DELTA: i64 = 2;

#[derive(Archive, Serialize, Deserialize)]
pub struct GossipHeader {
    nonce: [u8; 24],
}

#[repr(u8)]
#[derive(Archive, Serialize, Deserialize)]
pub enum Payload {
    FileRequest {
        hash: [u8; 32],
    } = 0,
    Provides {
        hash: [u8; 32],
        node_id: [u8; 32],
        #[rkyv(with= crate::util::DateTimeDef)]
        expire: DateTime<Utc>,
    } = 1,
}

#[derive(Archive, Serialize, Deserialize)]
pub struct Message {
    header: GossipHeader,
    payload: Vec<u8>,
}

pub struct GossipManager {
    topic: GossipSender,
    dir: SharedDirectory,
    protocol: SyncifyProtocol,
    handle: ManagerHandle,
}

impl GossipManager {
    pub async fn new(
        topic: GossipSender,
        dir: SharedDirectory,
        protocol: SyncifyProtocol,
        handle: ManagerHandle,
    ) -> Self {
        Self {
            topic,
            dir,
            protocol,
            handle,
        }
    }

    //noinspection RsTraitObligations
    pub async fn handle_events(
        &mut self,
        gossip_event: iroh_gossip::net::Event,
        local_tree: &mut HashTree,
        downloader: DownloaderHandle
    ) {
        info!("Dir {}: {:?}", self.dir.uuid(), gossip_event);
        match gossip_event {
            iroh_gossip::net::Event::Gossip(event) => match event {
                GossipEvent::Joined(node_id_vec) => {
                    let neighbors = &mut self.dir.write().await.neighbors;

                    for node_id in &node_id_vec {
                        Self::update_neighbors(neighbors, node_id);
                    }

                    self.handle.send(ManagerEvent::Sync(SyncEvent::TriggerSync(None))).await;
                }
                GossipEvent::NeighborUp(node_id) => {
                    let neighbors = &mut self.dir.write().await.neighbors;

                    Self::update_neighbors(neighbors, &node_id);
                }
                GossipEvent::NeighborDown(node_id) => {
                    let neighbors = &mut self.dir.write().await.neighbors;
                    *neighbors.get_mut(node_id.as_bytes()).unwrap() = false;
                }
                GossipEvent::Received(message) => {
                    if let Ok(msg) =
                        rkyv::from_bytes::<Message, rkyv::rancor::Error>(message.content.to_vec().as_slice())
                    {
                        let cipher = XChaCha20Poly1305::new(&Key::from(self.dir.verif_key.to_bytes()));
                        if let Ok(decrypted_payload) =
                            cipher.decrypt(&XNonce::from(msg.header.nonce), msg.payload.as_slice())
                        {
                            if let Ok(payload) =
                                rkyv::from_bytes::<Payload, rkyv::rancor::Error>(decrypted_payload.as_slice())
                            {
                                match payload {
                                    Payload::FileRequest { hash } => {
                                        if local_tree.map().contains_key(&Hash::from_bytes(hash)) {
                                            let expire = Utc::now()
                                                .checked_add_signed(TimeDelta::hours(PROVIDES_EXPIRATION_HOURS_DELTA))
                                                .unwrap();
                                            if let Ok(resp_msg) =
                                                self.create_message(Payload::Provides {
                                                    hash,
                                                    node_id: *self.protocol.endpoint().node_id().as_bytes(),
                                                    expire,
                                                })
                                            {
                                                if self.topic.broadcast(resp_msg).await.is_ok() {
                                                    downloader.send(DownloaderEvent::Provision(Hash::from_bytes(hash), expire)).await;
                                                } else {
                                                    warn!("Cannot broadcast Provides message!");
                                                }
                                            } else {
                                                warn!("Cannot create Provides message!");
                                            }
                                        }
                                    }
                                    Payload::Provides { hash, node_id, expire } => {
                                        if let Ok(node_id) = NodeId::from_bytes(&node_id) {
                                            downloader.send(DownloaderEvent::Provide(self.dir.uuid, node_id, Hash::from_bytes(hash), expire)).await;
                                        } else {
                                            warn!("Invalid NodeID!");
                                        }
                                    }
                                }
                            } else {
                                warn!("Cannot process received gossip message!");
                            }
                        } else {
                            warn!("Cannot process received gossip message!");
                        }
                    } else {
                        warn!("Cannot process received gossip message!");
                    }
                }
            },
            iroh_gossip::net::Event::Lagged => {}
        }
    }

    fn update_neighbors(neighbors: &mut HashMap<[u8; 32], bool>, node_id: &NodeId) {
        if !neighbors.keys().any(|e| node_id.as_bytes() == e) {
            neighbors.insert(*node_id.as_bytes(), true);
        } else {
            *neighbors.get_mut(node_id.as_bytes()).unwrap() = true;
        }
    }

    fn create_message(&self, payload: Payload) -> Result<Bytes, GossipError> {
        if let Ok(ser_payload) = rkyv::to_bytes::<rkyv::rancor::Error>(&payload) {
            let cipher = XChaCha20Poly1305::new(&Key::from(self.dir.verif_key.to_bytes()));
            let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

            if let Ok(encrypted_payload) = cipher.encrypt(&nonce, ser_payload.as_slice()) {
                let msg = Message {
                    header: GossipHeader {
                        nonce: <[u8; 24]>::try_from(nonce.as_slice()).unwrap(),
                    },
                    payload: encrypted_payload,
                };

                if let Ok(ser_msg) = rkyv::to_bytes::<rkyv::rancor::Error>(&msg) {
                    Ok(Bytes::from(ser_msg.to_vec()))
                } else {
                    Err(GossipError::Serialize)
                }
            } else {
                Err(GossipError::Encrypt)
            }
        } else {
            Err(GossipError::Encrypt)
        }
    }
}

#[derive(Error, Debug)]
pub enum GossipError {
    #[error("Cannot serialize")]
    Serialize,

    #[error("Unable to encrypt")]
    Encrypt,
}
