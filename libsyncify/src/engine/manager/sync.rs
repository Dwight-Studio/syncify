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
use crate::engine::manager::ManagerEvent;
use crate::engine::protocol::fsm::FiniteStateMachine;
use crate::engine::protocol::incoming_sync::IncomingSync;
use crate::engine::protocol::outgoing_sync::OutgoingSync;
use crate::engine::protocol::{SyncifyProtocol, SyncifyStream};
use crate::event::EventSender;
use blake3::Hash;
use log::{info, warn};
use std::ops::Deref;
use std::time::Duration;
use tokio::time::sleep;

pub const FSM_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SyncManager {
    sender: EventSender,
    dir: SharedDirectory,
    proto: SyncifyProtocol,
}

impl SyncManager {
    pub async fn new(sender: EventSender, dir: SharedDirectory, proto: SyncifyProtocol) -> Self {
        Self { sender, dir, proto }
    }

    pub async fn request_sync(&self, conn: SyncifyStream, hash: Hash) {
        *self.dir.initial_sync.write().await = true;

        let mut incoming_sync = IncomingSync::new(self.sender.clone(), conn, hash);

        if incoming_sync.step_until_finished(FSM_TIMEOUT).await {
            incoming_sync.step().await;
        } else {
            warn!("Timeout while processing sync event: RequestSync");
        }
    }

    pub async fn trigger_sync(&mut self, outgoing_opt: Option<OutgoingSync>) {
        match outgoing_opt {
            None => self.initial_sync().await,
            Some(outgoing) => Self::start_sync(outgoing).await,
        }
    }

    pub async fn initial_sync(&mut self) {
        let neighbors = self.dir.neighbors.read().await;
        for node in neighbors.deref() {
            if *node.1 {
                let outgoing = OutgoingSync::new(self.sender.clone(), self.dir.clone(), *node.0, self.proto.clone());

                if self.proto.node_id() > *node.0 {
                    Self::start_sync(outgoing).await;
                } else {
                    let dir = self.dir.clone();

                    tokio::spawn(async move {
                        sleep(Duration::from_secs(2)).await; // Wait 2 seconds for RequestDeltas
                        let received_request = *dir.initial_sync.read().await;

                        if !received_request {
                            info!("Initial sync: No sync request received. Initiating sync myself.");

                            dir.handle().await.send(ManagerEvent::TriggerSync(Some(outgoing))).await;
                        } else {
                            info!("Initial sync: Sync request received. No need to start sync.");
                            *dir.initial_sync.write().await = false
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
