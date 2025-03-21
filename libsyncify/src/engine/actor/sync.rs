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
use crate::engine::actor::{Event, SyncEvent};
use crate::engine::protocol::SyncifyProtocol;
use crate::engine::protocol::fsm::FiniteStateMachine;
use crate::engine::protocol::incoming_sync::IncomingSync;
use crate::engine::protocol::outgoing_sync::OutgoingSync;
use iroh::NodeId;
use iroh_gossip::net::GossipSender;
use log::{info, warn};
use std::time::Duration;
use tokio::time::sleep;

const FSM_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct SyncManager {
    pub(crate) topic: GossipSender,
    pub(crate) dir: SharedDirectory,
    pub(crate) prot: SyncifyProtocol,
}

impl SyncManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, syncify_prot: SyncifyProtocol) -> Self {
        Self {
            topic,
            dir,
            prot: syncify_prot,
        }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        match sync_event {
            SyncEvent::RequestSync(conn, hash) => {
                self.dir.write().await.received_initial_sync = true;

                let mut incoming_sync = IncomingSync::new(self.dir.clone(), conn.clone(), hash);

                if incoming_sync.step_until_finished(FSM_TIMEOUT).await {
                    incoming_sync.step().await;
                } else {
                    warn!("Timeout while processing sync event: RequestSync");
                }
            }
            SyncEvent::TriggerSync(outgoing_opt) => match outgoing_opt {
                None => self.initial_sync().await,
                Some(outgoing) => Self::start_sync(outgoing).await,
            },
        }
    }

    pub(crate) async fn initial_sync(&mut self) {
        let neighbors = self.dir.read().await.neighbors.clone();
        for node in neighbors {
            if node.1 {
                let node_id = NodeId::from_bytes(&node.0).unwrap();
                let outgoing = OutgoingSync::new(self.dir.clone(), node_id, self.prot.clone());

                if self.prot.endpoint().node_id() > node_id {
                    Self::start_sync(outgoing).await;
                } else {
                    let dir = self.dir.clone();

                    tokio::spawn(async move {
                        sleep(Duration::from_secs(2)).await; // Wait 2 seconds for RequestDeltas
                        let inner = dir.read().await;
                        let received_request = inner.received_initial_sync;

                        if !received_request {
                            info!("Initial sync: No sync request received. Initiating sync myself.");
                            
                            dir.handle().await.send(Event::Sync(SyncEvent::TriggerSync(Some(outgoing)))).await;
                        } else {
                            info!("Initial sync: Sync request received. No need to start sync.");
                        }
                    });
                }
                break;
            }
        }
    }

    async fn start_sync(mut outgoing_sync: OutgoingSync) {
        if outgoing_sync.step_until_finished(FSM_TIMEOUT).await {
            outgoing_sync.step().await;
        } else {
            warn!("Timeout while requesting sync");
        }
    }
}
