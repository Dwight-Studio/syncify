use tokio::task::JoinHandle;
use std::ops::Deref;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use notify::{Event, EventHandler};
use crate::store::StoreManager;

const NOTIFICATION_BUFFER_SIZE: usize = 1024;

/// Actor responsible to handle all filesystem events.
pub struct EventProcessor {
    task_handle: Option<JoinHandle<()>>,
    handle: EventProcessorHandle
}

impl EventProcessor {
    pub fn new(config: Arc<RwLock<StoreManager>>) -> Self {
        let (tx, rx) = mpsc::channel(NOTIFICATION_BUFFER_SIZE);

        Self {
            task_handle: Some(tokio::spawn(Self::handle_event(config, rx))),
            handle: EventProcessorHandle { tx }
        }
    }

    pub async fn handle_event(config: Arc<RwLock<StoreManager>>, mut rx: mpsc::Receiver<notify::Result<Event>>) {
        while let Some(event) = rx.recv().await {
            
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
        if let Err(error) = tokio::runtime::Handle::current().block_on(async {
            self.task_handle.take().unwrap().await
        }) {
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