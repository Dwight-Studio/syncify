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

use crate::engine::actor::SyncEvent;
use crate::engine::protocol::{SyncifyPacket, SyncifyProtocol};
use crate::engine::serial_state::SerialState;
use crate::engine::state::{State, MAX_LOADED_DELTAS};
use crate::SharedDirectory;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use iroh::NodeId;
use iroh_gossip::net::GossipSender;
use log::{error, info};
use std::collections::HashMap;

pub(crate) struct SyncManager {
    pub(crate) topic: GossipSender,
    pub(crate) dir: SharedDirectory,
    pub(crate) syncify_prot: SyncifyProtocol
}

impl SyncManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, syncify_prot: SyncifyProtocol) -> Self {
        Self { topic, dir, syncify_prot }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        match sync_event { 
            SyncEvent::RequestDeltas(mut conn, hash) => {
                let packet = {
                    if self.dir.inner.read().await.state.hash() == hash {
                        SyncifyPacket::Success {
                            pool: HashMap::new()
                        }
                    } else {
                        match self.dir.inner.read().await.state.after(hash, MAX_LOADED_DELTAS) {
                            None => { SyncifyPacket::Failed }
                            Some(state) => {
                                let serial_state = SerialState::from(&state);
                                SyncifyPacket::Success {
                                    pool: serial_state.pool()
                                }
                            }
                        }
                    }
                };

                conn.send_packet(self.dir.clone(), packet).await;
            },
            SyncEvent::TriggerInitialSync => {
                self.initial_sync().await;
            }
        }
    }
    
    pub(crate) async fn initial_sync(&self) {
        let inner_dir = self.dir.inner.read().await;
        for node in inner_dir.neighbors.clone() {
            if node.1 {
                let node_id = NodeId::from_bytes(&node.0).unwrap();
                info!("Attempting to sync with {}", &node_id);
                match self.syncify_prot.connect(node_id).await {
                    Ok(mut conn) => {
                        let request = SyncifyPacket::Request {
                            head: *inner_dir.state.hash().as_bytes()
                        };
                        conn.send_packet(self.dir.clone(), request).await;
                    }
                    Err(err) => {
                        error!("{}", err);
                    }
                }
                break;
            }
        }
    }
}
