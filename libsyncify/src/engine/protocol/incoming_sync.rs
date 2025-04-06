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
use crate::engine::manager::ManagerEvent;
use crate::engine::protocol::fsm::{FiniteStateMachine, ProtocolError};
use crate::engine::protocol::{SyncPacket, SyncifyPacket, SyncifyStream};
use crate::engine::state::{MAX_LOADED_DELTAS, StateError};
use crate::event::{EventSender, SyncEvent};
use log::{debug, error, info, warn};

#[derive(Eq, PartialEq)]
pub enum IncomingState {
    ReceivingRequest,
    SendingRequest,
    Finish,
    Failure(ProtocolError),
}

pub struct IncomingSync {
    state: IncomingState,
    sender: EventSender,
    connection: SyncifyStream,
    hash: blake3::Hash,
}

impl IncomingSync {
    pub fn new(sender: EventSender, connection: SyncifyStream, hash: blake3::Hash) -> Self {
        sender.send(SyncEvent::Incoming(connection.node_id()).wrap(connection.dir.uuid));
        Self {
            state: IncomingState::ReceivingRequest,
            sender,
            connection,
            hash,
        }
    }
}

impl FiniteStateMachine for IncomingSync {
    type State = IncomingState;

    async fn execute_step(&mut self) -> Result<Self::State, ProtocolError> {
        match &self.state {
            IncomingState::ReceivingRequest => {
                info!("Incoming sync request");
                let packet = {
                    match self
                        .connection
                        .dir
                        .state
                        .read()
                        .await
                        .clone_after(self.hash, MAX_LOADED_DELTAS)
                    {
                        Some(state) => SyncifyPacket::Sync(SyncPacket::Success { state }),
                        None => SyncifyPacket::Sync(SyncPacket::Failed),
                    }
                };

                if self.connection.send(&packet).await.is_err() {
                    Err(ProtocolError::SendFailed)
                } else {
                    Ok(IncomingState::SendingRequest)
                }
            }

            IncomingState::SendingRequest => {
                debug!("Incoming: SendingRequest");
                let packet = SyncPacket::Request {
                    head: self.connection.dir.state.read().await.hash(),
                };

                if let Ok(()) = self.connection.send(&SyncifyPacket::Sync(packet)).await {
                    match self.connection.recv().await {
                        Ok(packet) => {
                            if let SyncifyPacket::Sync(sync_packet) = packet {
                                match sync_packet {
                                    SyncPacket::Request { .. } => {}
                                    SyncPacket::Success { state: other_state } => {
                                        debug!("Incoming: Receiving state");
                                        //debug!("Incoming: Receiving state: \n{}", state);

                                        let mutations = match self
                                            .connection
                                            .dir
                                            .state
                                            .write()
                                            .verify_accept_all(other_state, &self.connection.dir.read_key)
                                            .await
                                        {
                                            Ok(m) => m,
                                            Err(e) => {
                                                return match e {
                                                    StateError::InvalidSignature => {
                                                        self.sender.send(
                                                            SyncEvent::Unverified(self.connection.node_id)
                                                                .wrap(self.connection.dir.uuid),
                                                        );
                                                        Err(ProtocolError::InvalidSignature)
                                                    }
                                                    _ => Err(ProtocolError::Unexpected),
                                                };
                                            }
                                        };

                                        self.connection
                                            .dir
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

                            if self.connection.close().await.is_err() {
                                return Err(ProtocolError::Unexpected);
                            }
                            Ok(IncomingState::Finish)
                        }
                        Err(e) => {
                            error!("Received failed: {e}");
                            Err(ProtocolError::ReceiveFailed)
                        }
                    }
                } else {
                    Err(ProtocolError::SendFailed)
                }
            }

            IncomingState::Finish => {
                debug!("Incoming: Finished");
                Ok(IncomingState::Finish)
            }

            IncomingState::Failure(error) => {
                warn!("Incoming: Failure ({:?})", error);
                Ok(IncomingState::Failure(*error))
            }
        }
    }

    fn finished(&self) -> bool {
        match self.state {
            IncomingState::ReceivingRequest | IncomingState::SendingRequest => false,
            IncomingState::Finish | IncomingState::Failure(_) => true,
        }
    }

    fn current_state(&self) -> &Self::State {
        &self.state
    }

    fn error(&self) -> Option<ProtocolError> {
        match self.state {
            IncomingState::Failure(error) => Some(error),
            _ => None,
        }
    }

    fn transition(&mut self, state: Self::State) {
        self.state = state;
    }

    fn transition_error(&mut self, error: ProtocolError) {
        self.state = IncomingState::Failure(error);
    }
}
