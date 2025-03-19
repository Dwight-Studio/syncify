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
use crate::engine::actor::SyncEvent;
use crate::engine::protocol::SyncifyProtocol;
use iroh::NodeId;
use iroh_gossip::net::GossipSender;
use log::{info, warn};
use std::time::Duration;
use tokio::time::sleep;
use crate::engine::protocol::fsm::FiniteStateMachine;
use crate::engine::protocol::incoming_sync::IncomingSync;
use crate::engine::protocol::outgoing_sync::OutgoingSync;

const FSM_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SyncManager {
    pub(crate) topic: GossipSender,
    pub(crate) dir: SharedDirectory,
    pub(crate) syncify_prot: SyncifyProtocol,
}

impl SyncManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, syncify_prot: SyncifyProtocol) -> Self {
        Self {
            topic,
            dir,
            syncify_prot,
        }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        match &sync_event {
            SyncEvent::RequestDeltas(conn, hash) => {
                self.dir.inner.write().await.received_sync = true;
                
                let mut incoming_sync = IncomingSync::new(self.dir.clone(), conn.clone(), *hash);
                
                while !incoming_sync.finished() {
                    let timeout_result = tokio::time::timeout(FSM_TIMEOUT, incoming_sync.step()).await;

                    if timeout_result.is_err() {
                        warn!("Timeout while processing sync event: RequestDeltas");
                        break;
                    }
                }
                incoming_sync.step().await;
            }
        }
    }

    pub(crate) async fn initial_sync(&mut self) {
        for node in self.dir.inner.read().await.neighbors.clone() {
            if node.1 {
                let node_id = NodeId::from_bytes(&node.0).unwrap();
                let outgoing = OutgoingSync::new(self.dir.clone(), node_id, self.syncify_prot.clone());

                if self.syncify_prot.endpoint.node_id() > node_id {
                    Self::start_sync(outgoing).await;
                } else {
                    let dir = self.dir.clone();
                    
                    tokio::spawn(async move {
                        sleep(Duration::from_secs(2)).await; // Wait 2 seconds for RequestDeltas
                        let received_request = dir.inner.read().await.received_sync;

                        if !received_request {
                            info!("Outgoing: No sync request received. Initiating sync myself.");
                            Self::start_sync(outgoing).await;
                        } else {
                            info!("Outgoing: Sync request received. No need to start sync.");
                        }
                    });
                }
                break;
            }
        }
    }

    async fn start_sync(mut outgoing_sync: OutgoingSync) {
        while !outgoing_sync.finished() {
            let timeout_result = tokio::time::timeout(FSM_TIMEOUT, outgoing_sync.step()).await;
            
            if timeout_result.is_err() {
                warn!("Timeout while requesting sync");
                break;
            }
        }
        outgoing_sync.step().await;
    }
}
