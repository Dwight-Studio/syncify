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
use crate::engine::protocol::fsm::{FiniteStateMachine, ProtocolError};
use crate::engine::protocol::{SyncPacket, SyncifyPacket, SyncifyProtocol, SyncifyStream};
use crate::engine::state::{MAX_LOADED_DELTAS, StateError};
use blake3::Hash;
use iroh::NodeId;
use log::{debug, error, info, warn};
use std::sync::Arc;
use tokio::sync::RwLock;
// TODO: Add provision database sync

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
    proto: Arc<RwLock<SyncifyProtocol>>,
    connection: Option<SyncifyStream>,
}

impl OutgoingSync {
    pub fn new(dir: SharedDirectory, node_id: NodeId, proto: Arc<RwLock<SyncifyProtocol>>) -> Self {
        Self {
            state: OutgoingState::Connecting,
            dir,
            node_id,
            connection: None,
            proto,
        }
    }
}

impl FiniteStateMachine for OutgoingSync {
    type State = OutgoingState;

    async fn execute_step(&mut self) -> Result<Self::State, ProtocolError> {
        match &self.state {
            OutgoingState::Connecting => {
                info!("Outgoing: Requesting sync to {}", self.node_id);
                debug!("ACQUIRING PROTO");
                let mut proto = self.proto.write().await;
                if let Ok(conn) = proto.open_stream(&self.dir, self.node_id).await {
                    self.connection = Some(conn);
                    drop(proto);
                    debug!("RELEASING PROTO");
                    Ok(OutgoingState::SendingRequest)
                } else {
                    drop(proto);
                    debug!("RELEASING PROTO");
                    Err(ProtocolError::ConnectionFailed)
                }
            }

            OutgoingState::SendingRequest => {
                debug!("Outgoing: SendingRequest");
                let packet = SyncPacket::Request {
                    head: *self.dir.state.read().await.hash().as_bytes(),
                };

                if let Some(ref mut conn) = self.connection {
                    if let Ok(()) = conn.send(&SyncifyPacket::Sync(packet)).await {
                        match conn.recv().await {
                            Ok(packet) => {
                                if let SyncifyPacket::Sync(sync_packet) = packet {
                                    match sync_packet {
                                        SyncPacket::Request { .. } => {}
                                        SyncPacket::Success { state: other_state } => {
                                            debug!("Outgoing: Receiving state");
                                            //debug!("Outgoing: Receiving state\n{}", state);

                                            let mutations = self
                                                .dir
                                                .state
                                                .write()
                                                .verify_accept_all(other_state, &self.dir.read_key)
                                                .await
                                                .map_err(|e| match e {
                                                    StateError::InvalidSignature => ProtocolError::InvalidSignature,
                                                    _ => ProtocolError::Unexpected,
                                                })?;

                                            self.dir
                                                .handle()
                                                .await
                                                .send(ManagerEvent::ApplyRemoteMutations(mutations))
                                                .await;
                                        }
                                        SyncPacket::Failed => {}
                                    }
                                } else {
                                    return Err(ProtocolError::Unexpected);
                                }

                                Ok(OutgoingState::ReceivingRequest)
                            }
                            Err(e) => {
                                error!("Received failed: {e}");
                                Err(ProtocolError::ReceiveFailed)
                            }
                        }
                    } else {
                        Err(ProtocolError::SendFailed)
                    }
                } else {
                    Err(ProtocolError::ConnectionFailed)
                }
            }

            OutgoingState::ReceivingRequest => {
                debug!("Outgoing: ReceivingRequest");

                if let Some(ref mut conn) = self.connection {
                    let hash = if let Ok(request) = conn.recv().await {
                        if let SyncifyPacket::Sync(sync_packet) = request {
                            if let SyncPacket::Request { head } = sync_packet {
                                Hash::from_bytes(head)
                            } else {
                                return Err(ProtocolError::Unexpected);
                            }
                        } else {
                            return Err(ProtocolError::Unexpected);
                        }
                    } else {
                        return Err(ProtocolError::ReceiveFailed);
                    };
                    let packet = {
                        match self.dir.state.read().await.clone_after(hash, MAX_LOADED_DELTAS) {
                            Some(state) => SyncPacket::Success { state },
                            None => SyncPacket::Failed,
                        }
                    };

                    if let Ok(()) = conn.send(&SyncifyPacket::Sync(packet)).await {
                        if conn.close().await.is_err() {
                            return Err(ProtocolError::Unexpected);
                        }
                        Ok(OutgoingState::Finish)
                    } else {
                        Err(ProtocolError::SendFailed)
                    }
                } else {
                    Err(ProtocolError::ConnectionFailed)
                }
            }

            OutgoingState::Finish => {
                debug!("Outgoing: Finished");
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
