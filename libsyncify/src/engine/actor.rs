use crate::engine::state::HashTree;
use crate::engine::{fs, gossip};
use crate::SharedDirectory;
use futures::{AsyncRead, Sink, StreamExt};
use iroh_gossip::net::{GossipReceiver, GossipSender, GossipTopic};
use log::{debug, error, info};
use notify::{EventHandler, Watcher};
use std::io::Error;
use std::ops::Deref;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const EVENT_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events for a [`SharedDirectory`].
pub struct DirectoryManager {
    _watcher: notify::RecommendedWatcher,
    join_handle: Option<JoinHandle<()>>,
    handle: DirectoryManagerHandle,
}

impl DirectoryManager {
    pub fn new(
        dir: SharedDirectory,
        topic: GossipTopic,
    ) -> Result<Self, notify::Error> {
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
        let join_handle = Some(tokio::spawn(Self::handle_event(rx, dir, gossip_tx)));

        // Create and configure watcher
        let mut _watcher = notify::recommended_watcher(handle.clone())?;
        _watcher.watch(path.as_path(), notify::RecursiveMode::Recursive)?;

        Ok(Self { _watcher, join_handle, handle })
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
    ) {
        // First, verify that the current state correspond to the what's in memory
        let old_tree = match dir.state.write().await.hash_tree().await {
            Ok(tree) => tree,
            Err(e) => {
                error!("Unable to create hash tree from saved state: {e}");
                rx.close();
                return
            }
        };

        match HashTree::from_disk(dir.path().as_path()) {
            Ok(tree) => {
                info!("{:?}", old_tree);
                info!("{:?}", tree);
                if tree == old_tree {
                    info!("State is up-to-date");
                } else {
                    info!("State is out-of-date");
                }
            }
            Err(e) => {
                error!("Unable to create hash tree: {e}");
                rx.close();
                return
            }
        }

        // Then, process the events
        while let Some(result) = rx.recv().await {
            match result {
                Event::FileSystem(fs_event) => {
                    fs::handle_events(&dir, &topic, fs_event).await
                },

                Event::Gossip(gossip_event) => {
                    gossip::handle_events(&dir, &topic, gossip_event).await
                }

                Event::Shutdown => {
                    rx.close();
                    debug!("Closing event channel for {}", dir.uuid())
                },
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

impl EventHandler for DirectoryManagerHandle {
    fn handle_event(&mut self, raw_event: notify::Result<notify::Event>) {
        if let Ok(event) = raw_event {
            if let Err(error) = self.tx.blocking_send(Event::FileSystem(event)) {
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
        match self.tx.blocking_send(Event::Gossip(item)) {
            Ok(()) => Ok(()),
            Err(e) => Err(iroh_gossip::net::Error::from(e)),
        }
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
    FileSystem(notify::Event),
    Gossip(iroh_gossip::net::Event),
    Shutdown
}