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
use crate::engine::state::MAX_LOADED_DELTAS;
use crate::SharedDirectory;
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
                    info!("Requested: Hash from Requester is {:?}. Hash from Requested is: {:?}", hash, self.dir.inner.read().await.state.hash());
                    if self.dir.inner.read().await.state.hash() == hash {
                        SyncifyPacket::Success {
                            pool: HashMap::new()
                        }
                    } else {
                        match self.dir.inner.read().await.state.clone_after(hash, MAX_LOADED_DELTAS) {
                            None => { SyncifyPacket::Failed }
                            Some(state) => {
                                SyncifyPacket::Success {
                                    pool: state.pool().clone()
                                }
                            }
                        }
                    }
                };
                info!("Requested: Handled RequestDeltas event! Sending success/failed response...");
                conn.send_packet(self.dir.clone(), packet).await.unwrap();
                info!("Requested: Sent success/failed response!");
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
                info!("Requester: Attempting to sync with {}", &node_id);
                match self.syncify_prot.connect(node_id).await {
                    Ok(mut conn) => {
                        let request = SyncifyPacket::Request {
                            head: *inner_dir.state.hash().as_bytes()
                        };
                        let dir = self.dir.clone();
                        tokio::spawn(async move {
                            info!("Requester: Sending request...");
                            conn.send_packet(dir.clone(), request).await.unwrap();
                            info!("Requester: Finished sending request!");
                            match conn.receive_packet(dir.clone()).await {
                                Ok(packet) => {info!("Requester: {:?}: {:?}", packet.0, packet.1)}
                                Err(err) => { error!("{}", err); }
                            }
                            info!("Requester: Received success/failed response!");
                        });
                        
                    }
                    Err(err) => {
                        error!("Requester: Error while connecting {}", err);
                    }
                }
                break;
            }
        }
    }
}
