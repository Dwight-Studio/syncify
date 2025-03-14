use crate::store::StoreManager;
use crate::SharedDirectory;
use log::info;
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::EventKind::{Create, Modify, Remove};
use notify::{Event, EventHandler, Watcher};
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

const NOTIFICATION_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events for a [`SharedDirectory`].
pub struct DirectoryManager {
    watcher: notify::RecommendedWatcher,
    handle: DirectoryManagerHandle,
}

impl DirectoryManager {
    pub fn new(
        store: Arc<RwLock<StoreManager>>,
        dir: SharedDirectory,
    ) -> Result<Self, notify::Error> {
        info!("Initializing directory manager for {}", dir.uuid());

        // Initiate channel
        let (tx, rx) = mpsc::channel(NOTIFICATION_BUFFER_SIZE);
        let handle = DirectoryManagerHandle { tx };

        // Spawn new thread
        let path = dir.path();
        tokio::spawn(Self::handle_event(store, dir, rx));

        // Create and configure watcher
        let mut watcher = notify::recommended_watcher(handle.clone())?;
        watcher.watch(path.as_path(), notify::RecursiveMode::Recursive)?;

        Ok(Self { watcher, handle })
    }

    pub async fn handle_event(
        store: Arc<RwLock<StoreManager>>,
        dir: SharedDirectory,
        mut rx: mpsc::Receiver<notify::Result<Event>>,
    ) {
        while let Some(result) = rx.recv().await {
            if let Ok(event) = result {
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
            }
        }
        info!("Dropped directory manager for {}", dir.uuid());
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
    tx: mpsc::Sender<notify::Result<Event>>,
}

impl EventHandler for DirectoryManagerHandle {
    fn handle_event(&mut self, event: notify::Result<Event>) {
        if let Err(error) = self.tx.blocking_send(event) {
            log::error!("Failed to send event: {error}");
        };
    }
}
