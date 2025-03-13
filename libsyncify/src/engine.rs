use crate::engine::fs::EventProcessor;
use crate::engine::protocol::SyncifyProtocol;
use crate::get_app_dir;
use crate::store::StoreManager;
use iroh::Endpoint;
use iroh::protocol::Router;
use iroh_blobs::net_protocol::Blobs;
use iroh_gossip::net::Gossip;
use notify::Watcher;
use std::io;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

mod fs;
mod protocol;
mod state;

const DOWNLOAD_DIRNAME: &str = "download";
const DATABASE_DIRNAME: &str = "database";

pub struct Engine {
    router: Router,
    watcher: notify::RecommendedWatcher,
    processor: EventProcessor,
}

/// Synchronization engine.
impl Engine {
    /// Construct new instance.
    pub async fn new(store: Arc<RwLock<StoreManager>>) -> Result<Self, EngineError> {
        let endpoint = Endpoint::builder()
            .secret_key(store.read().await.secret_key.clone())
            .alpns(vec![iroh_blobs::ALPN.to_vec(), iroh_gossip::ALPN.to_vec()])
            .discovery_n0()
            .discovery_local_network()
            .bind()
            .await
            .map_err(EngineError::EndpointInit)?;

        // Router
        let builder = Router::builder(endpoint);

        // Blobs protocol
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

        let syncify_prot = SyncifyProtocol {
            store: store.clone(),
        };

        // File watcher
        let processor = EventProcessor::new(store.clone());
        let watcher =
            notify::recommended_watcher(processor.clone()).map_err(EngineError::CannotWatch)?;

        let mut engine = Self {
            router: builder
                .accept(protocol::SYNCIFY_ALPN, syncify_prot)
                .accept(iroh_blobs::ALPN, blobs)
                .accept(iroh_gossip::ALPN, gossip)
                .spawn()
                .await
                .map_err(EngineError::RouterInit)?,
            watcher,
            processor,
        };

        for dir in &store.read().await.get_all_dirs() {
            engine.add_watched_directory(&dir.path()).await?
        }

        Ok(engine)
    }

    /// Gracefully shutdown.
    pub async fn shutdown(self) {
        self.router.shutdown().await.unwrap();
        drop(self);
    }

    pub async fn add_watched_directory(&mut self, path: &Path) -> Result<(), EngineError> {
        self.watcher
            .watch(path, notify::RecursiveMode::Recursive)
            .map_err(EngineError::CannotWatch)?;
        Ok(())
    }

    pub async fn remove_watched_directory(&mut self, path: &Path) -> Result<(), EngineError> {
        self.watcher.unwatch(path).map_err(EngineError::CannotWatch)
    }

    #[cfg(debug_assertions)]
    pub fn get_node_endpoint(&self) -> &Endpoint {
        self.router.endpoint()
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
    CannotUnwatch(notify::Error),
}
