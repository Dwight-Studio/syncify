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
use crate::engine::actor::Event;
use crate::engine::protocol::fsm::{FiniteStateMachine, ProtocolError};
use crate::engine::protocol::{SyncifyConnection, SyncifyPacket, SyncifyProtocol};
use crate::engine::state::{MAX_LOADED_DELTAS, StateError};
use blake3::Hash;
use iroh::NodeId;
use log::{info, warn};

#[derive(PartialEq, Debug)]
pub enum OutgoingState {
    Connecting,
    SendingRequest,
    ReceivingRequest,
    Finish,
    Failure(ProtocolError),
}

pub struct OutgoingSync {
    state: OutgoingState,
    dir: SharedDirectory,
    node_id: NodeId,
    protocol: SyncifyProtocol,
    connection: Option<SyncifyConnection>,
}

impl OutgoingSync {
    pub fn new(dir: SharedDirectory, node_id: NodeId, protocol: SyncifyProtocol) -> Self {
        Self {
            state: OutgoingState::Connecting,
            dir,
            node_id,
            protocol,
            connection: None,
        }
    }
}

impl FiniteStateMachine for OutgoingSync {
    type State = OutgoingState;

    async fn execute_step(&mut self) -> Result<Self::State, ProtocolError> {
        match &self.state {
            OutgoingState::Connecting => {
                info!("Outgoing: Requesting sync to {}", self.node_id);
                if let Ok(conn) = self.protocol.connect(self.node_id).await {
                    self.connection = Some(conn);
                    Ok(OutgoingState::SendingRequest)
                } else {
                    Err(ProtocolError::ConnectionFailed)
                }
            }

            OutgoingState::SendingRequest => {
                info!("Outgoing: SendingRequest");
                let packet = SyncifyPacket::Request {
                    head: *self.dir.read().await.state.hash().as_bytes(),
                };

                let mut conn = self.connection.clone().unwrap();

                if let Ok(()) = conn.send_packet(self.dir.clone(), packet).await {
                    if let Ok(packet) = conn.receive_packet(self.dir.clone()).await {
                        match packet {
                            SyncifyPacket::Request { .. } => {}
                            SyncifyPacket::Success { state } => {
                                info!("Outgoing: Receiving state\n{}", state);
                                let mut inner = self.dir.write().await;
                                let mutations =
                                    inner
                                        .state
                                        .verify_and_add(state, self.dir.clone())
                                        .map_err(|e| match e {
                                            StateError::InvalidSignature => ProtocolError::InvalidSignature,
                                            _ => ProtocolError::Unexpected,
                                        })?;

                                if let Some(handle) = &inner.handle {
                                    handle.send(Event::ApplyRemoteMutations(mutations)).await;
                                }
                            }
                            SyncifyPacket::Failed => {}
                        }

                        Ok(OutgoingState::ReceivingRequest)
                    } else {
                        Err(ProtocolError::ReceiveFailed)
                    }
                } else {
                    Err(ProtocolError::SendFailed)
                }
            }

            OutgoingState::ReceivingRequest => {
                info!("Outgoing: ReceivingRequest");
                let mut conn = self.connection.clone().unwrap();

                let hash = if let Ok(request) = conn.receive_packet(self.dir.clone()).await {
                    if let SyncifyPacket::Request { head } = request {
                        Hash::from_bytes(head)
                    } else {
                        return Err(ProtocolError::Unexpected);
                    }
                } else {
                    return Err(ProtocolError::ReceiveFailed);
                };
                let packet = {
                    match self.dir.read().await.state.clone_after(hash, MAX_LOADED_DELTAS) {
                        Some(state) => SyncifyPacket::Success { state },
                        None => SyncifyPacket::Failed,
                    }
                };

                if let Ok(()) = conn.send_packet(self.dir.clone(), packet).await {
                    Ok(OutgoingState::Finish)
                } else {
                    Err(ProtocolError::SendFailed)
                }
            }

            OutgoingState::Finish => {
                info!("Outgoing: Finished");
                Ok(OutgoingState::Finish)
            }

            OutgoingState::Failure(error) => {
                warn!("Outgoing: Failure ({:?})", error);
                Ok(OutgoingState::Failure(*error))
            }
        }
    }

    fn finished(&self) -> bool {
        match self.state {
            OutgoingState::Connecting | OutgoingState::SendingRequest | OutgoingState::ReceivingRequest => false,
            OutgoingState::Finish | OutgoingState::Failure(_) => true,
        }
    }

    fn current_state(&self) -> &Self::State {
        &self.state
    }

    fn error(&self) -> Option<ProtocolError> {
        match self.state {
            OutgoingState::Failure(error) => Some(error),
            _ => None,
        }
    }

    fn transition(&mut self, state: Self::State) {
        self.state = state;
    }

    fn transition_error(&mut self, error: ProtocolError) {
        self.state = OutgoingState::Failure(error);
    }
}
