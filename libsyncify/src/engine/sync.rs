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
use std::sync::Arc;
use crate::SharedDirectory;
use crate::engine::actor::SyncEvent;
use crate::engine::protocol::{SyncifyPacket, SyncifyProtocol};
use iroh::NodeId;
use iroh_gossip::net::GossipSender;
use log::{error, info, warn};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::sleep;
use crate::engine::protocol::fsm::FiniteStateMachine;
use crate::engine::protocol::incoming_sync::IncomingSync;
use crate::engine::protocol::outgoing_sync::OutgoingSync;

const FSM_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SyncManager {
    pub(crate) topic: GossipSender,
    pub(crate) dir: SharedDirectory,
    pub(crate) syncify_prot: SyncifyProtocol,
    outgoing: Arc<RwLock<Option<OutgoingSync>>>,
}

impl SyncManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, syncify_prot: SyncifyProtocol) -> Self {
        Self {
            topic,
            dir,
            syncify_prot,
            outgoing: None
        }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        match &sync_event {
            SyncEvent::RequestDeltas(_, _) => {
                if let Some(mut outgoing) = self.outgoing.write().await.take() {
                    let timeout_result = tokio::time::timeout(FSM_TIMEOUT, outgoing.step(Some(sync_event))).await;

                    if let Ok(result) = timeout_result {
                        if !outgoing.finished() {
                            self.outgoing.write().aw = Some(outgoing);
                        }
                    } else {
                        warn!("Timout while processing sync event: RequestDeltas");
                    }
                } else {
                    let mut fsm = IncomingSync::new(self.dir.clone(), self.syncify_prot.clone());
                    let timeout_result = tokio::time::timeout(FSM_TIMEOUT, fsm.step(Some(sync_event))).await;

                    if let Ok(result) = timeout_result {

                    } else {
                        warn!("Timout while processing sync event: RequestDeltas");
                    }
                }
            }
        }
    }

    pub(crate) async fn initial_sync(&mut self) {
        let inner_dir = self.dir.inner.read().await;
        for node in inner_dir.neighbors.clone() {
            if node.1 {
                let node_id = NodeId::from_bytes(&node.0).unwrap();
                let dir = self.dir.clone();
                let syncify_prot = self.syncify_prot.clone();
                let outgoing_ref = self.outgoing.clone();

                if self.syncify_prot.endpoint.node_id() > node_id {
                    let mut outgoing = OutgoingSync::new(self.dir.clone(), node_id, self.syncify_prot.clone());
                    outgoing.step(None).await;
                    self.outgoing = Some(Arc::new(RwLock::new(outgoing)));

                } else {
                    tokio::spawn(async move {
                        sleep(Duration::from_secs(2)).await; // Wait 2 seconds for RequestDeltas
                        let received_request = dir.inner.read().await.received_sync;

                        if !received_request {
                            info!("Requester: No sync request received. Initiating sync myself.");
                            let mut outgoing = OutgoingSync::new(dir, node_id, syncify_prot);
                            outgoing.step(None).await;
                            outgoing_ref = Some(Arc::new(RwLock::new(outgoing)));
                        } else {
                            info!("Requester: Sync request received. No need to start sync.");
                        }
                    });
                }
                break;
            }
        }
    }

    async fn start_sync(node_id: NodeId, syncify_prot: SyncifyProtocol, dir: SharedDirectory) {
        info!("Requester: Attempting to sync with {}", &node_id);
        match syncify_prot.connect(node_id).await {
            Ok(mut conn) => {
                let request = SyncifyPacket::Request {
                    head: *dir.inner.read().await.state.hash().as_bytes(),
                };
                info!("Requester: Sending request...");
                conn.send_packet(dir.clone(), request).await.unwrap();
                info!("Requester: Finished sending request!");
                match conn.receive_packet(dir.clone()).await {
                    Ok(packet) => {
                        info!("Requester: {:?}: {:?}", packet.0, packet.1);
                        /*match conn.receive_packet(dir.clone()).await {
                            Ok(packet) => {}
                            Err(err) => {
                                error!("{}", err)
                            }
                        }*/
                    }
                    Err(err) => {
                        error!("{}", err);
                    }
                }
                info!("Requester: Received success/failed response!");
            }
            Err(err) => {
                error!("Requester: Error while connecting {}", err);
            }
        }
    }
}
