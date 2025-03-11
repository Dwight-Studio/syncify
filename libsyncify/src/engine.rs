use std::sync::mpsc;
use std::{io, thread};
use std::collections::HashMap;
use std::path::PathBuf;
use iroh::Endpoint;
use iroh::protocol::Router;
use iroh_blobs::net_protocol::Blobs;
use iroh_gossip::net::Gossip;
use notify::Watcher;
use thiserror::Error;
use crate::{get_app_dir, SharedDirectory};
use crate::config::{SyncifyConfig};

const DOWNLOAD_DIRNAME: &str = "download";
const DATABASE_DIRNAME: &str = "database";
const FILESYSTEM_EVENT_BUF_SIZE: usize = 1024;

pub struct Engine {
    router: Router,
    watcher: notify::RecommendedWatcher,
    event_handler: EventHandler,
}

/// Synchronization engine.
impl Engine {
    /// Construct new instance.
    pub async fn new(config: &SyncifyConfig) -> Result<Self, EngineError> {
        let endpoint = Endpoint::builder()
            .secret_key(config.secret_key.clone())
            .alpns(vec![
                iroh_blobs::ALPN.to_vec(),
                iroh_gossip::ALPN.to_vec(),
            ])
            .discovery_n0()
            .discovery_local_network()
            .user_data_for_discovery(config.user_data.clone())
            .bind()
            .await
            .map_err(EngineError::EndpointInit)?;

        // Router
        let builder = Router::builder(endpoint);

        // Blobs protocole
        let download_dir = get_app_dir().join(DOWNLOAD_DIRNAME);

        if !download_dir.exists() {
            tokio::fs::create_dir_all(&download_dir)
                .await
                .map_err(EngineError::MakeDir)?
        }

        let blobs = Blobs::persistent(download_dir)
            .await
            .map_err(EngineError::BlobsInit)?
            .build(builder.endpoint());

        // Gossip protocol
        let gossip = Gossip::builder()
            .spawn(builder.endpoint().clone())
            .await
            .map_err(EngineError::GossipInit)?;

        let (events_tx, events_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let watcher = notify::recommended_watcher(events_tx).map_err(EngineError::WatcherInit)?;
        
        let event_handler = EventHandler(Some(thread::spawn(move || handle_events(events_rx))));

        let mut engine = Self {
            router: builder
                .accept(iroh_blobs::ALPN, blobs)
                .accept(iroh_gossip::ALPN, gossip)
                .spawn()
                .await
                .map_err(EngineError::RouterInit)?,
            watcher,
            event_handler
        };

        for folder in &config.data.dirs {
            engine.add_watched_directory(folder)
                .await?
        }

        Ok(engine)
    }

    /// Gracefully shutdown.
    pub async fn shutdown(self) {
        self.router.shutdown().await.unwrap();
        drop(self);
    }

    pub async fn add_watched_directory(&mut self, dir: &SharedDirectory) -> Result<(), EngineError> {
        self.watcher.watch(&PathBuf::from(dir.path.clone()), notify::RecursiveMode::Recursive)
            .map_err(EngineError::CannotWatch)?;
        Ok(())
    }

    pub async fn remove_watched_directory(&mut self, dir: &SharedDirectory) -> Result<(), EngineError> {
        self.watcher.unwatch(&PathBuf::from(dir.path.clone())).map_err(EngineError::CannotWatch)
    }
}

struct EventHandler(Option<thread::JoinHandle<()>>);

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.0.take().unwrap().join().unwrap();
    }
}

fn handle_events(events_rx: mpsc::Receiver<notify::Result<notify::Event>>) {
    for res in events_rx {
        match res {
            Ok(event) => println!("event: {:?}", event),
            Err(e) => println!("watch error: {:?}", e),
        }
    }
}

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("Filesystem error: Cannot create directory ({0})")]
    MakeDir(io::Error),

    #[error("Endpoint error: {0}")]
    EndpointInit(anyhow::Error),

    #[error("Blobs error: {0}")]
    BlobsInit(anyhow::Error),
    
    #[error("Gossip error: {0}")]
    GossipInit(iroh_gossip::net::Error),

    #[error("Docs error: {0}")]
    DocsInit(anyhow::Error),

    #[error("Router error: {0}")]
    RouterInit(anyhow::Error),

    #[error("Filesystem Watcher error: {0}")]
    WatcherInit(notify::Error),

    #[error("Unable to watch directory (is it a network filesystem?): {0}")]
    CannotWatch(notify::Error),

    #[error("Unable to unwatch directory: {0}")]
    CannotUnwatch(notify::Error)
}