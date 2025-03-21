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
use crate::SharedDirectory;
use crate::engine::protocol::outgoing_sync::OutgoingSync;
use crate::engine::protocol::{SyncifyConnection, SyncifyProtocol};
use crate::engine::state::Mutation;
use sync::SyncManager;
use chrono::{DateTime, Utc};
use futures::{Sink, StreamExt};
use iroh_gossip::net::{GossipSender, GossipTopic};
use log::{debug, error, info};
use notify::{EventHandler, RecommendedWatcher, Watcher};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use blake3::Hash;
use iroh_blobs::net_protocol::Blobs;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use crate::engine::actor::fs::{FileSystemManager, Job};
use crate::engine::actor::gossip::{GossipManager, Provided};

pub mod fs;
pub mod gossip;
pub mod sync;

const EVENT_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events for a [`SharedDirectory`].
pub struct DirectoryManager {
    _watcher: Option<notify::RecommendedWatcher>,
    join_handle: Option<JoinHandle<()>>,
    handle: DirectoryManagerHandle,
}

impl DirectoryManager {
    pub async fn new(dir: SharedDirectory, topic: GossipTopic, blobs: Blobs<iroh_blobs::store::fs::Store>, syncify_prot: SyncifyProtocol) -> Result<Self, notify::Error> {
        info!("Initializing directory manager for {}", dir.uuid());

        // Initiate channel
        let (tx, rx) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = DirectoryManagerHandle { tx };
        dir.write().await.handle = Some(handle.clone());

        // Plug gossip stream into the channel
        let (gossip_tx, gossip_rx) = topic.split();
        let forward = gossip_rx.forward(handle.clone());
        tokio::spawn(forward);

        // Spawn new thread
        let path = dir.path();
        let join_handle = Some(tokio::spawn(Self::handle_event(
            rx,
            dir.clone(),
            gossip_tx,
            blobs,
            syncify_prot,
            handle.clone(),
        )));

        // Create and configure watcher
        let mut _watcher = None;
        if dir.is_read_only() {
            info!("{} is in read only", dir.uuid)
        } else {
            let mut watcher = notify::recommended_watcher(handle.clone())?;
            watcher.watch(path.as_path(), notify::RecursiveMode::Recursive)?;
            info!(
                "{} is writable, attached '{:?}' file watcher",
                dir.uuid,
                RecommendedWatcher::kind()
            );
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
    async fn handle_event(
        mut rx: mpsc::Receiver<Event>,
        dir: SharedDirectory,
        topic: GossipSender,
        blobs: Blobs<iroh_blobs::store::fs::Store>,
        protocol: SyncifyProtocol,
        handle: DirectoryManagerHandle,
    ) {
        // Shared variables
        let mut jobs_buffer: Vec<Arc<RwLock<Job>>> = Vec::new();
        let mut last_tree = dir.read().await.state.hash_tree().clone();
        let mut provides_map: HashMap<[u8; 32], Vec<Provided>> = HashMap::new();
        
        let mut fs_manager = FileSystemManager::new(topic.clone(), blobs.clone(), dir.clone(), &mut last_tree).await;
        let mut gossip_manager = GossipManager::new(topic.clone(), dir.clone(), protocol.clone(), handle.clone()).await;
        let mut sync_manager = SyncManager::new(topic.clone(), dir.clone(), protocol.clone()).await;

        // Process the event
        while let Some(event) = rx.recv().await {
            match event {
                // FileSystem
                Event::FileSystem(fs_event, timestamp) => fs_manager.handle_events(fs_event, timestamp, &mut jobs_buffer, &mut last_tree).await,
                Event::ApplyLocalMutations => fs_manager.apply_local_mutations(&mut last_tree).await,
                Event::GenerateJobs(mutations) => fs_manager.generate_jobs(&mut jobs_buffer, mutations, &mut provides_map).await,

                // Gossip
                Event::Gossip(gossip_event) => gossip_manager.handle_events(gossip_event, &mut last_tree, &mut provides_map).await,
                Event::RequestFileProviders(hash) => {/* Awesome method */}

                // Protocol
                Event::Sync(sync_event) => sync_manager.handle_events(sync_event).await,

                // Actor
                Event::Shutdown => {
                    rx.close();
                    debug!("Closing event channel for {}", dir.uuid())
                }
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
    pub async fn send(&self, event: Event) {
        if let Err(e) = self.tx.send(event).await {
            error!("Error sending to actor: {e}");
        }
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
            fut.send(Event::Gossip(item)).await.unwrap();
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

/// Event to control the [`DirectoryManager`] actor.
pub enum Event {
    // FileSystem
    FileSystem(notify::Event, DateTime<Utc>),
    ApplyLocalMutations,
    GenerateJobs(Vec<Mutation>),

    // Gossip
    Gossip(iroh_gossip::net::Event),
    RequestFileProviders(Hash),

    // Protocol
    Sync(SyncEvent),

    // Actor
    Shutdown,
}

pub enum SyncEvent {
    RequestSync(SyncifyConnection, blake3::Hash),
    TriggerSync(Option<OutgoingSync>),
}
