use crate::engine::fs::DirectoryManager;
use crate::engine::protocol::SyncifyProtocol;
use crate::engine::EngineError::AlreadyWatched;
use crate::store::StoreManager;
use crate::{get_app_dir, SharedDirectory};
use iroh::protocol::Router;
use iroh::Endpoint;
use iroh_blobs::net_protocol::Blobs;
use iroh_gossip::net::Gossip;
use log::info;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod fs;
pub mod protocol;
pub mod state;

const DOWNLOAD_DIRNAME: &str = "download";

pub struct Engine {
    router: Router,
    managers: HashMap<Uuid, DirectoryManager>,
}

/// Synchronization engine.
impl Engine {
    /// Construct new instance.
    pub async fn new(store: Arc<RwLock<StoreManager>>) -> Result<Self, EngineError> {
        info!("Initializing engine");
        let endpoint = Endpoint::builder()
            .secret_key(store.read().await.secret_key())
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

        let mut engine = Self {
            router: builder
                .accept(protocol::SYNCIFY_ALPN, syncify_prot)
                .accept(iroh_blobs::ALPN, blobs)
                .accept(iroh_gossip::ALPN, gossip)
                .spawn()
                .await
                .map_err(EngineError::RouterInit)?,
            managers: HashMap::new(),
        };

        for dir in &store.read().await.get_all_dirs() {
            engine.add_watched_directory(store.clone(), dir).await?
        }

        Ok(engine)
    }

    /// Gracefully shutdown.
    pub async fn shutdown(self) {
        info!("Shutting down engine");
        self.router.shutdown().await.unwrap();
        for ref mut entries in self.managers {
            let (_, manager) = entries;
            manager.shutdown().await;
        }
    }

    /// Create [`DirectoryManager`] actor for a [`SharedDirectory`].
    pub async fn add_watched_directory(
        &mut self,
        store: Arc<RwLock<StoreManager>>,
        dir: &SharedDirectory,
    ) -> Result<(), EngineError> {
        if !self.managers.contains_key(&dir.uuid()) {
            // Create manager
            let uuid = dir.uuid();
            let manager =
                DirectoryManager::new(dir.clone()).map_err(EngineError::CannotWatch)?;

            self.managers.insert(uuid, manager);
            Ok(())
        } else {
            Err(AlreadyWatched(dir.uuid()))
        }
    }

    pub async fn remove_watched_directory(
        &mut self,
        dir: &SharedDirectory,
    ) -> Result<(), EngineError> {
        info!("Removing directory manager for {}", dir.uuid());
        if self.managers.contains_key(&dir.uuid()) {
            self.managers.remove(&dir.uuid()).unwrap();
            Ok(())
        } else {
            Err(AlreadyWatched(dir.uuid()))
        }
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

    #[error("Filesystem Watcher error (is it a network filesystem?): {0}")]
    CannotWatch(notify::Error),

    #[error("Unable to unwatch directory: {0}")]
    CannotUnwatch(notify::Error),

    #[error("Directory is already watched: {0}")]
    AlreadyWatched(Uuid),
}
