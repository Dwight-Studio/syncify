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
use crate::engine::job::{DownloadJob, LocalProvision};
use crate::engine::manager::fs::FileSystemManager;
use crate::engine::manager::gossip::GossipManager;
use crate::engine::protocol::outgoing_sync::OutgoingSync;
use crate::engine::protocol::{SyncifyProtocol, SyncifyStream};
use crate::engine::state::Mutation;
use crate::event::{EngineEvent, EventSender};
use crate::store::lock::StoreLock;
use blake3::Hash;
use futures::{Sink, StreamExt};
use iroh_gossip::net::{GossipSender, GossipTopic};
use log::{debug, error, info};
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use sync::SyncManager;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub mod fs;
pub mod gossip;
pub mod sync;

/// Size of the [`ManagerEvent`] buffer for [`Manager`].
pub const EVENT_BUFFER_SIZE: usize = 1024;
/// Interval between each filesystem polling.
pub const WATCHER_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Actor responsible to handle all sync events for a [`SharedDirectory`].
#[derive(Debug)]
pub struct Manager {
    watcher_join_handle: Option<JoinHandle<()>>,
    join_handle: Option<JoinHandle<()>>,
    handle: ManagerHandle,
}

impl Manager {
    pub async fn new(
        sender: EventSender,
        dir: SharedDirectory,
        topic: GossipTopic,
        downloader: DownloaderHandle,
        proto: SyncifyProtocol,
    ) -> Self {
        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = ManagerHandle { tx };
        *dir.handle.write().await = Some(handle.clone());

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
                        handle.send(ManagerEvent::Poll).await;
                    }
                }))
            }
        };

        // Spawn new thread
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
            sender,
            dir.clone(),
            gossip_tx,
            downloader,
            handle.clone(),
            proto,
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
            let _ = join_handle.await;
        }
        let _ = self.tx.send(ManagerEvent::Shutdown).await;
        let _ = self.join_handle.take().unwrap().await;
    }

    /// Main method of the [`Manager`].
    async fn handle_event(
        mut rx: mpsc::Receiver<ManagerEvent>,
        sender: EventSender,
        dir: SharedDirectory,
        topic: GossipSender,
        downloader: DownloaderHandle,
        handle: ManagerHandle,
        proto: SyncifyProtocol,
    ) {
        let mut fs_manager =
            FileSystemManager::new(sender.clone(), dir.clone(), handle.clone(), downloader.clone()).await;
        let mut gossip_manager = GossipManager::new(
            sender.clone(),
            dir.clone(),
            topic.clone(),
            proto.clone(),
            handle.clone(),
            downloader.clone(),
        )
        .await;
        let mut sync_manager = SyncManager::new(sender.clone(), dir.clone(), proto.clone()).await;

        // Notify
        info!("Started directory manager for {}", dir.uuid());
        sender.send(EngineEvent::ManagerStarted(dir.uuid()).wrap());

        // Process the event
        while let Some(event) = rx.recv().await {
            match event {
                // FileSystem
                ManagerEvent::Poll => fs_manager.poll().await,
                ManagerEvent::ApplyRemoteMutations(mutations) => fs_manager.apply_remote_mutations(mutations).await,
                ManagerEvent::DownloadFinished(job) => fs_manager.download_finished(job).await,

                // Gossip
                ManagerEvent::Gossip(gossip_event) => gossip_manager.handle_events(gossip_event).await,
                ManagerEvent::RequestProvision(hash) => gossip_manager.request_provision(hash).await,
                ManagerEvent::ConfirmLocalProvision(provision) => {
                    gossip_manager.confirm_local_provision(provision).await
                }
                ManagerEvent::BroadcastUpdate => gossip_manager.notify_changes().await,

                // Protocol
                ManagerEvent::RequestSync(conn, hash) => sync_manager.request_sync(conn, hash).await,
                ManagerEvent::TriggerSync(outgoing_opt) => sync_manager.trigger_sync(outgoing_opt).await,

                // Actor
                ManagerEvent::Shutdown => {
                    rx.close();
                    debug!("Closing manager event channel for {}", dir.uuid());
                    sender.send(EngineEvent::ManagerStopped(dir.uuid()).wrap());
                }
            }
        }
        info!("Finished manager event processing for {}", dir.uuid());
        sender.send(EngineEvent::ManagerFinished(dir.uuid()).wrap());
    }
}

impl Deref for Manager {
    type Target = ManagerHandle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Handle to a [`Manager`].
#[derive(Debug, Clone)]
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
    Poll,
    ApplyRemoteMutations(Vec<Mutation>),
    DownloadFinished(StoreLock<DownloadJob>),

    // Gossip
    Gossip(iroh_gossip::net::Event),
    RequestProvision(Hash),
    ConfirmLocalProvision(LocalProvision),
    BroadcastUpdate,

    // Protocol
    RequestSync(SyncifyStream, Hash),
    TriggerSync(Option<OutgoingSync>),

    // Actor
    Shutdown,
}
