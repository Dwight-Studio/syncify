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
use crate::engine::downloader::DownloaderHandle;
use crate::engine::manager::fs::FileSystemManager;
use crate::engine::manager::gossip::GossipManager;
use crate::engine::protocol::outgoing_sync::OutgoingSync;
use crate::engine::protocol::SyncifyConnection;
use crate::engine::state::{HashTree, Mutation};
use blake3::Hash;
use chrono::{DateTime, Utc};
use futures::{Sink, StreamExt};
use iroh_gossip::net::{GossipSender, GossipTopic};
use log::{debug, error, info, warn};
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use iroh::Endpoint;
use sync::SyncManager;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub mod fs;
pub mod gossip;
pub mod sync;

/// Size of the event buffer for [`Manager`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Interval between each filesystem polling.
pub const WATCHER_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Actor responsible to handle all sync events for a [`SharedDirectory`].
pub struct Manager {
    watcher_join_handle: Option<JoinHandle<()>>,
    join_handle: Option<JoinHandle<()>>,
    handle: ManagerHandle,
}

impl Manager {
    pub async fn new(
        dir: SharedDirectory,
        topic: GossipTopic,
        ep: Endpoint,
        downloader: DownloaderHandle,
    ) -> Self {
        info!("Initializing directory manager for {}", dir.uuid());

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = ManagerHandle { tx };
        dir.write().await.handle = Some(handle.clone());

        // Plug gossip stream into the channel
        let (gossip_tx, gossip_rx) = topic.split();
        let forward = gossip_rx.forward(handle.clone());
        tokio::spawn(forward);

        let watcher_join_handle = {
            if dir.is_read_only() {
                None
            } else {
                let handle = handle.clone();
                Some(tokio::spawn(async move {
                    let mut interval = tokio::time::interval(WATCHER_POLL_INTERVAL);

                    loop {
                        interval.tick().await;
                        handle.send(ManagerEvent::PollFiles).await;
                    }
                }))
            }
        };

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
            dir.clone(),
            gossip_tx,
            ep,
            downloader,
            handle.clone(),
        )));

        Self {
            watcher_join_handle,
            join_handle,
            handle,
        }
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        if let Some(join_handle) = self.watcher_join_handle.take() {
            join_handle.abort();
            if let Err(e) = join_handle.await {
                warn!("Cannot join watcher thread: {e}");
            }
        }
        self.tx.send(ManagerEvent::Shutdown).await.unwrap();
        self.join_handle.take().unwrap().await.unwrap();
    }

    /// Main method of the [`Manager`].
    async fn handle_event(
        mut rx: mpsc::Receiver<ManagerEvent>,
        dir: SharedDirectory,
        topic: GossipSender,
        ep: Endpoint,
        downloader: DownloaderHandle,
        handle: ManagerHandle,
    ) {
        // Shared variables
        let mut local_tree = {
            match HashTree::from_disk(&dir) {
                Ok(tree) => tree,
                Err(e) => {
                    error!("Cannot get file tree from disk for {}: {e}", dir.uuid());
                    return;
                }
            }
        };

        let mut fs_manager = FileSystemManager::new(topic.clone(), downloader.clone(), dir.clone()).await;
        let mut gossip_manager = GossipManager::new(topic.clone(), dir.clone(), ep.clone(), handle.clone()).await;
        let mut sync_manager = SyncManager::new(topic.clone(), dir.clone(), ep.clone()).await;

        // Process the event
        while let Some(event) = rx.recv().await {
            match event {
                // FileSystem
                ManagerEvent::PollFiles => fs_manager.poll(&mut local_tree).await,
                ManagerEvent::ApplyRemoteMutations(mutations) => {
                    fs_manager.apply_remote_mutations(mutations, &mut local_tree).await
                }
                ManagerEvent::UpdateLocalTree(mutation) => {
                    fs_manager.update_local_tree(mutation, &mut local_tree).await
                }

                // Gossip
                ManagerEvent::Gossip(gossip_event) => {
                    gossip_manager
                        .handle_events(gossip_event, &mut local_tree, downloader.clone())
                        .await
                }
                ManagerEvent::ConfirmLocalProvision(hash, expiration) => {
                    gossip_manager.confirm_local_provision(hash, expiration).await
                }

                // Protocol
                ManagerEvent::Sync(sync_event) => sync_manager.handle_events(sync_event).await,

                // Actor
                ManagerEvent::Shutdown => {
                    rx.close();
                    debug!("Closing manager event channel for {}", dir.uuid())
                }
            }
        }
        info!("Finished manager event processing for {}", dir.uuid());
    }
}

impl Deref for Manager {
    type Target = ManagerHandle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Handle to a [`Manager`].
#[derive(Clone)]
pub struct ManagerHandle {
    tx: mpsc::Sender<ManagerEvent>,
}

impl ManagerHandle {
    /// Send an [`ManagerEvent`] to the actor.
    pub async fn send(&self, event: ManagerEvent) {
        if let Err(e) = self.tx.send(event).await {
            error!("Error sending to manager: {e}");
        }
    }

    /// Check if the actor is still alive.
    pub fn is_alive(&self) -> bool {
        self.tx.is_closed()
    }
}

impl Sink<iroh_gossip::net::Event> for ManagerHandle {
    type Error = iroh_gossip::net::Error;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.tx.is_closed() {
            Poll::Ready(Err(iroh_gossip::net::Error::ReceiverClosed))
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(self: Pin<&mut Self>, item: iroh_gossip::net::Event) -> Result<(), Self::Error> {
        let fut = self.tx.clone();
        tokio::spawn(async move {
            fut.send(ManagerEvent::Gossip(item)).await.unwrap();
        });
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// Event to control the [`Manager`].
pub enum ManagerEvent {
    // FileSystem
    PollFiles,
    ApplyRemoteMutations(Vec<Mutation>),
    UpdateLocalTree(Mutation),

    // Gossip
    Gossip(iroh_gossip::net::Event),
    ConfirmLocalProvision(Hash, DateTime<Utc>),

    // Protocol
    Sync(SyncEvent),

    // Actor
    Shutdown,
}

pub enum SyncEvent {
    RequestSync(SyncifyConnection, Hash),
    TriggerSync(Option<OutgoingSync>),
}
