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

use crate::engine::fs::FileSystemManager;
use crate::engine::gossip::GossipManager;
use crate::engine::protocol::{SyncifyConnection, SyncifyProtocol};
use crate::engine::sync::SyncManager;
use crate::SharedDirectory;
use futures::{Sink, StreamExt};
use iroh_gossip::net::{GossipSender, GossipTopic};
use log::{debug, info};
use notify::{EventHandler, RecommendedWatcher, Watcher};
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;
use tokio::task::JoinHandle;
use crate::engine::protocol::outgoing_sync::OutgoingSync;

const EVENT_BUFFER_SIZE: usize = 1024;
const MUTATIONS_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// Actor responsible to handle all filesystem events for a [`SharedDirectory`].
pub struct DirectoryManager {
    _watcher: Option<notify::RecommendedWatcher>,
    join_handle: Option<JoinHandle<()>>,
    handle: DirectoryManagerHandle,
}

impl DirectoryManager {
    pub fn new(dir: SharedDirectory, topic: GossipTopic, syncify_prot: SyncifyProtocol) -> Result<Self, notify::Error> {
        info!("Initializing directory manager for {}", dir.uuid());

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DirectoryManagerHandle { tx };

        // Plug gossip stream into the channel
        let (gossip_tx, gossip_rx) = topic.split();
        let forward = gossip_rx.forward(handle.clone());
        tokio::spawn(forward);

        // Spawn new thread
        let path = dir.path();
        let join_handle = Some(tokio::spawn(Self::handle_event(rx, dir.clone(), gossip_tx, syncify_prot, handle.clone())));

        // Create and configure watcher
        let mut _watcher = None;
        if dir.is_read_only() {
            info!("{} is in read only", dir.uuid)
        } else {
            let mut watcher = notify::recommended_watcher(handle.clone())?;
            watcher.watch(path.as_path(), notify::RecursiveMode::Recursive)?;
            info!("{} is writable, attached '{:?}' file watcher", dir.uuid, RecommendedWatcher::kind());
            _watcher = Some(watcher);
        }

        Ok(Self {
            _watcher,
            join_handle,
            handle,
        })
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        self.tx.send(Event::Shutdown).await.unwrap();
        self.join_handle.take().unwrap().await.unwrap();
    }

    /// Main method of the actor.
    pub async fn handle_event(
        mut rx: mpsc::Receiver<Event>,
        dir: SharedDirectory,
        topic: GossipSender,
        syncify_prot: SyncifyProtocol,
        handle: DirectoryManagerHandle
    ) {
        let mut fs_manager = FileSystemManager::new(topic.clone(), dir.clone()).await;
        let mut gossip_manager = GossipManager::new(topic.clone(), dir.clone(), handle.clone()).await;
        let mut sync_manager = SyncManager::new(topic.clone(), dir.clone(), syncify_prot).await;

        // Process the event
        loop {
            // Check if the mutation buffer is empty,
            let result = if fs_manager.mutations_buffer.is_empty() {
                // If so, wait for event
                rx.recv().await
            } else {
                // If not, wait for event with a timeout
                match tokio::time::timeout(MUTATIONS_FLUSH_TIMEOUT, rx.recv()).await {
                    // Return if there's an event
                    Ok(result) => result,
                    // Flush if not
                    Err(_) => {
                        fs_manager.apply_mutations().await;
                        continue;
                    }
                }
            };
            
            if let Some(event) = result {
                match event {
                    // FileSystem
                    Event::FileSystem(fs_event, timestamp) => fs_manager.handle_events(fs_event, timestamp).await,
                    Event::ApplyMutations => fs_manager.apply_mutations().await,

                    // Gossip
                    Event::Gossip(gossip_event) => gossip_manager.handle_events(gossip_event).await,

                    // Protocol
                    Event::Sync(sync_event) => sync_manager.handle_events(sync_event).await,

                    // Actor
                    Event::Shutdown => {
                        rx.close();
                        debug!("Closing event channel for {}", dir.uuid())
                    }
                }
            } else {
                break;
            }
        }
        info!("Finished event processing for {}", dir.uuid());
    }
}

impl Deref for DirectoryManager {
    type Target = DirectoryManagerHandle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Handle to a [`DirectoryManager`].
#[derive(Clone)]
pub struct DirectoryManagerHandle {
    tx: mpsc::Sender<Event>,
}

impl DirectoryManagerHandle {
    pub async fn send(&self, event: Event) -> Result<(), SendError<Event>> {
        self.tx.send(event).await
    }
}

impl EventHandler for DirectoryManagerHandle {
    fn handle_event(&mut self, raw_event: notify::Result<notify::Event>) {
        if let Ok(event) = raw_event {
            if let Err(error) = self.tx.blocking_send(Event::FileSystem(event, Utc::now())) {
                log::error!("Failed to send event: {error}");
            };
        }
    }
}

impl Sink<iroh_gossip::net::Event> for DirectoryManagerHandle {
    type Error = iroh_gossip::net::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.tx.is_closed() {
            Poll::Ready(Err(iroh_gossip::net::Error::ReceiverClosed))
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn start_send(self: Pin<&mut Self>, item: iroh_gossip::net::Event) -> Result<(), Self::Error> {
        let fut = self.tx.clone();
        tokio::spawn(async move {
            fut.send(Event::Gossip(item)).await;
        });
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// Event to control the [`DirectoryManager`] actor.
pub enum Event {
    // FileSystem
    FileSystem(notify::Event, DateTime<Utc>),
    ApplyMutations,

    // Gossip
    Gossip(iroh_gossip::net::Event),

    // Protocol
    Sync(SyncEvent),

    // Actor
    Shutdown,
}

pub enum SyncEvent {
    RequestSync(SyncifyConnection, blake3::Hash),
    TriggerSync(Option<OutgoingSync>)
}
