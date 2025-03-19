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
use crate::engine::state::MAX_LOADED_DELTAS;
use std::collections::HashMap;
use blake3::Hash;
use iroh::NodeId;
use log::info;
use crate::engine::protocol::{SyncifyConnection, SyncifyPacket, SyncifyProtocol};
use crate::engine::protocol::fsm::{FiniteStateMachine, ProtocolError};
use crate::SharedDirectory;

#[derive(PartialEq, Debug)]
pub enum OutgoingState {
    Initialize,
    Connected,
    WaitForRequest,
    Finish,
    Failure,
}

pub struct OutgoingSync {
    state: OutgoingState,
    dir: SharedDirectory,
    node_id: NodeId,
    protocol: SyncifyProtocol,
    connection: Option<SyncifyConnection>
}

impl OutgoingSync {
    pub fn new(dir: SharedDirectory, node_id: NodeId, protocol: SyncifyProtocol) -> Self {
        Self {
            state: OutgoingState::Initialize,
            dir,
            node_id,
            protocol,
            connection: None,
        }
    }
}

impl FiniteStateMachine for OutgoingSync {
    type State = OutgoingState;

    async fn step(&mut self) -> Result<(), ProtocolError> {
        match &self.state {

            OutgoingState::Initialize => {
                info!("Outgoing: Requesting sync to {}", self.node_id);
                if let Ok(conn) = self.protocol.connect(self.node_id).await {
                    self.state = OutgoingState::Connected;
                    self.connection = Some(conn);
                    Ok(())
                } else {
                    self.state = OutgoingState::Failure;
                    Err(ProtocolError::ConnectionFailed)
                }
            }

            OutgoingState::Connected => {
                info!("Outgoing: Connected");
                let packet = SyncifyPacket::Request {
                    head: *self.dir.inner.read().await.state.hash().as_bytes()
                };
                
                let mut conn = self.connection.clone().unwrap();
                
                if let Ok(()) = conn.send_packet(self.dir.clone(), packet).await {
                    if let Ok((uuid, packet)) = conn.receive_packet(self.dir.clone()).await {
                        
                        // TODO: Process Success/Failed packet
                        
                        self.state = OutgoingState::WaitForRequest;
                        Ok(())
                    } else {
                        self.state = OutgoingState::Failure;
                        Err(ProtocolError::ReceiveFailed)
                    }
                } else {
                    self.state = OutgoingState::Failure;
                    Err(ProtocolError::SendFailed)
                }
            }

            OutgoingState::WaitForRequest => {
                info!("Outgoing: WaitForRequest");
                let mut conn = self.connection.clone().unwrap();
                
                let hash = if let Ok(request) = conn.receive_packet(self.dir.clone()).await {
                    if let SyncifyPacket::Request { head } = request.1 {
                        Hash::from_bytes(head)
                    } else {
                        self.state = OutgoingState::Failure;
                        return Err(ProtocolError::Unexpected)
                    }
                } else {
                    self.state = OutgoingState::Failure;
                    return Err(ProtocolError::ReceiveFailed)
                };
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
                    self.state = OutgoingState::Finish;
                    Ok(())
                } else {
                    self.state = OutgoingState::Failure;
                    Err(ProtocolError::SendFailed)
                }
            }

            OutgoingState::Finish | OutgoingState::Failure => {
                info!("Outgoing: Finished");
                Ok(())
            }
        }
    }

    fn finished(&self) -> bool {
        match self.state {
            OutgoingState::Initialize | OutgoingState::Connected | OutgoingState::WaitForRequest => false,
            OutgoingState::Finish | OutgoingState::Failure => true
        }
    }

    fn current_state(&self) -> &Self::State {
        &self.state
    }
}