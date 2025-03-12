use crate::store::StoreManager;
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::EventKind::{Create, Modify, Remove};
use notify::{Event, EventHandler};
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;

const NOTIFICATION_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events.
pub struct EventProcessor {
    task_handle: Option<JoinHandle<()>>,
    handle: EventProcessorHandle,
}

impl EventProcessor {
    pub fn new(config: Arc<RwLock<StoreManager>>) -> Self {
        let (tx, rx) = mpsc::channel(NOTIFICATION_BUFFER_SIZE);

        Self {
            task_handle: Some(tokio::spawn(Self::handle_event(config, rx))),
            handle: EventProcessorHandle { tx },
        }
    }

    pub async fn handle_event(
        config: Arc<RwLock<StoreManager>>,
        mut rx: mpsc::Receiver<notify::Result<Event>>,
    ) {
        while let Some(result) = rx.recv().await {
            if let Ok(event) = result {
                match event.kind {
                    Create(kind) => match kind {
                        CreateKind::Any => {}
                        CreateKind::File => {}
                        CreateKind::Folder => {}
                        CreateKind::Other => {}
                    },
                    Modify(kind) => match kind {
                        ModifyKind::Any => {}
                        ModifyKind::Data(_) => {}
                        ModifyKind::Metadata(_) => {}
                        ModifyKind::Name(_) => {}
                        ModifyKind::Other => {}
                    },
                    Remove(kind) => match kind {
                        RemoveKind::Any => {}
                        RemoveKind::File => {}
                        RemoveKind::Folder => {}
                        RemoveKind::Other => {}
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

impl Drop for EventProcessor {
    fn drop(&mut self) {
        if let Err(error) =
            futures::executor::block_on(async { self.task_handle.take().unwrap().await })
        {
            log::error!("Failed to join thread: {error}");
        }
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
