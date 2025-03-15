use crate::engine::state::HashTree;
use crate::SharedDirectory;
use log::{error, info};
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::EventKind::{Create, Modify, Remove};
use notify::{Event, EventHandler, Watcher};
use std::ops::Deref;
use tokio::sync::{mpsc};
use tokio::task::JoinHandle;

const NOTIFICATION_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events for a [`SharedDirectory`].
pub struct DirectoryManager {
    watcher: notify::RecommendedWatcher,
    join_handle: Option<JoinHandle<()>>,
    handle: DirectoryManagerHandle,
}

impl DirectoryManager {
    pub fn new(
        dir: SharedDirectory,
    ) -> Result<Self, notify::Error> {
        info!("Initializing directory manager for {}", dir.uuid());

        // Initiate channel
        let (tx, rx) = mpsc::channel(NOTIFICATION_BUFFER_SIZE);
        let handle = DirectoryManagerHandle { tx };

        // Spawn new thread
        let path = dir.path();
        let join_handle = Some(tokio::spawn(Self::handle_event(dir, rx)));

        // Create and configure watcher
        let mut watcher = notify::recommended_watcher(handle.clone())?;
        watcher.watch(path.as_path(), notify::RecursiveMode::Recursive)?;

        Ok(Self { watcher, join_handle, handle })
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        self.tx.send(None).await.unwrap();
        self.join_handle.take().unwrap().await.unwrap();
    }

    pub async fn handle_event(
        dir: SharedDirectory,
        mut rx: mpsc::Receiver<Option<Event>>,
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
            if let Some(event) = result {
                info!("Dir {}: {:?}", dir.uuid(), event.kind);
                match event.kind {
                    Create(kind) => match kind {
                        CreateKind::File => {}
                        _ => {}
                    },
                    Modify(kind) => match kind {
                        ModifyKind::Data(_) => {}
                        ModifyKind::Name(_) => {}
                        _ => {}
                    },
                    Remove(kind) => match kind {
                        RemoveKind::File => {}
                        _ => {}
                    },
                    _ => continue,
                }
            } else {
                // If received None, close the channel
                rx.close();
                info!("Closing event channel for {}", dir.uuid())
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
    tx: mpsc::Sender<Option<Event>>,
}

impl EventHandler for DirectoryManagerHandle {
    fn handle_event(&mut self, raw_event: notify::Result<Event>) {
        if let Ok(event) = raw_event {
            if let Err(error) = self.tx.blocking_send(Some(event)) {
                log::error!("Failed to send event: {error}");
            };
        }
    }
}