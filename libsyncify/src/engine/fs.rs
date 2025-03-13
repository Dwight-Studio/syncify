use crate::store::StoreManager;
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::EventKind::{Create, Modify, Remove};
use notify::{Event, EventHandler};
use std::ops::Deref;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;

const NOTIFICATION_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events.
pub struct EventProcessor {
    handle: EventProcessorHandle,
}

impl EventProcessor {
    pub fn new(store: Arc<RwLock<StoreManager>>) -> Self {
        let (tx, rx) = mpsc::channel(NOTIFICATION_BUFFER_SIZE);
        tokio::spawn(Self::handle_event(store, rx));
        Self {
            handle: EventProcessorHandle { tx },
        }
    }

    pub async fn handle_event(
        store: Arc<RwLock<StoreManager>>,
        mut rx: mpsc::Receiver<notify::Result<Event>>,
    ) {
        while let Some(result) = rx.recv().await {
            if let Ok(event) = result {
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
    }
}

impl Deref for EventProcessor {
    type Target = EventProcessorHandle;

    fn deref(&self) -> &Self::Target {
        &self.handle
    }
}

/// Handle to a [`EventProcessor`].
#[derive(Clone)]
pub struct EventProcessorHandle {
    tx: mpsc::Sender<notify::Result<Event>>,
}

impl EventHandler for EventProcessorHandle {
    fn handle_event(&mut self, event: notify::Result<Event>) {
        if let Err(error) = self.tx.blocking_send(event) {
            log::error!("Failed to send event: {error}");
        };
    }
}
