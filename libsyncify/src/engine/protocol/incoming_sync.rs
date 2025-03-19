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
use iroh::NodeId;
use log::info;
use crate::engine::actor::SyncEvent;
use crate::engine::protocol::{SyncifyConnection, SyncifyPacket, SyncifyProtocol};
use crate::engine::protocol::fsm::{FiniteStateMachine, ProtocolError};
use crate::engine::state::MAX_LOADED_DELTAS;
use crate::SharedDirectory;

enum State {
    Requested,
    Finish,
    Failure,
}

pub struct IncomingSync {
    state: State,
    dir: SharedDirectory,
    protocole: SyncifyProtocol,
}

impl IncomingSync {
    pub fn new(dir: SharedDirectory, protocole: SyncifyProtocol) -> Self {
        Self {
            state: State::Requested,
            protocole,
            dir,
        }
    }
}

impl FiniteStateMachine for IncomingSync {
    async fn step(&mut self, event: Option<SyncEvent>) -> Result<(), ProtocolError> {
        match &self.state {

            State::Requested => {
                if let Some(SyncEvent::RequestDeltas(mut conn, hash)) = event {
                    info!("Incoming sync request from {}", conn.remote());
                    let packet = {
                        if self.dir.inner.read().await.state.hash() == hash {
                            SyncifyPacket::Success { pool: HashMap::new() }
                        } else {
                            match self.dir.inner.read().await.state.clone_after(hash, MAX_LOADED_DELTAS) {
                                Some(state) => SyncifyPacket::Success {
                                    pool: state.pool().clone(),
                                },
                                None => SyncifyPacket::Failed,
                            }
                        }
                    };

                    if let Ok(()) = conn.send_packet(self.dir.clone(), packet).await {
                        let packet = SyncifyPacket::Request {
                            head: *self.dir.inner.read().await.state.hash().as_bytes()
                        };

                        if let Ok(()) = conn.send_packet(self.dir.clone(), packet).await {
                            if let Ok((uuid, packet)) = conn.receive_packet(self.dir.clone()).await {

                                // TODO: Process Success/Failed packet

                                self.state = State::Finish;
                                Ok(())
                            } else {
                                self.state = State::Failure;
                                Err(ProtocolError::ReceiveFailed)
                            }
                        } else {
                            self.state = State::Failure;
                            Err(ProtocolError::SendFailed)
                        }
                    } else {
                        self.state = State::Failure;
                        Err(ProtocolError::SendFailed)
                    }
                } else {
                    Ok(())
                }
            }

            State::Finish | State::Failure => {
                Ok(())
            }
        }
    }

    fn finished(&self) -> bool {
        match self.state {
            State::Requested => false,
            State::Finish | State::Failure => true
        }
    }
}