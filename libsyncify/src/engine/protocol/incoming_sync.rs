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
use log::info;
use crate::engine::protocol::{SyncifyConnection, SyncifyPacket};
use crate::engine::protocol::fsm::{FiniteStateMachine, ProtocolError};
use crate::engine::state::MAX_LOADED_DELTAS;
use crate::SharedDirectory;

#[derive(PartialEq)]
pub enum IncomingState {
    ReceivingRequest,
    SendingRequest,
    Finish,
    Failure,
}

pub struct IncomingSync {
    state: IncomingState,
    dir: SharedDirectory,
    connection: SyncifyConnection,
    hash: blake3::Hash
}

impl IncomingSync {
    pub fn new(dir: SharedDirectory, connection: SyncifyConnection, hash: blake3::Hash) -> Self {
        Self {
            state: IncomingState::ReceivingRequest,
            dir,
            connection,
            hash
        }
    }
}

impl FiniteStateMachine for IncomingSync {
    type State = IncomingState;

    async fn step(&mut self) -> Result<(), ProtocolError> {
        match &self.state {

            IncomingState::ReceivingRequest => {
                info!("Incoming sync request from {}", self.connection.remote());
                let packet = {
                    if self.dir.inner.read().await.state.hash() == self.hash {
                        SyncifyPacket::Success { pool: HashMap::new() }
                    } else {
                        match self.dir.inner.read().await.state.clone_after(self.hash, MAX_LOADED_DELTAS) {
                            Some(state) => SyncifyPacket::Success {
                                pool: state.pool().clone(),
                            },
                            None => SyncifyPacket::Failed,
                        }
                    }
                };

                if self.connection.send_packet(self.dir.clone(), packet).await.is_err() {
                    self.state = IncomingState::Failure;
                    Err(ProtocolError::SendFailed)
                } else {
                    self.state = IncomingState::SendingRequest;
                    Ok(())
                }
            }
            
            IncomingState::SendingRequest => {
                info!("Incoming: SendingRequest");
                let packet = SyncifyPacket::Request {
                    head: *self.dir.inner.read().await.state.hash().as_bytes()
                };

                if let Ok(()) = self.connection.send_packet(self.dir.clone(), packet).await {
                    if let Ok((uuid, packet)) = self.connection.receive_packet(self.dir.clone()).await {

                        // TODO: Process Success/Failed packet

                        self.state = IncomingState::Finish;
                        Ok(())
                    } else {
                        self.state = IncomingState::Failure;
                        Err(ProtocolError::ReceiveFailed)
                    }
                } else {
                    self.state = IncomingState::Failure;
                    Err(ProtocolError::SendFailed)
                }
            }

            IncomingState::Finish | IncomingState::Failure => {
                info!("Incoming: Finished");
                Ok(())
            }
        }
    }

    fn finished(&self) -> bool {
        match self.state {
            IncomingState::ReceivingRequest | IncomingState::SendingRequest => false,
            IncomingState::Finish | IncomingState::Failure => true
        }
    }

    fn current_state(&self) -> &Self::State {
        &self.state
    }
}