use crate::engine::actor::DirectoryManager;
use crate::engine::protocol::SyncifyProtocol;
use crate::engine::EngineError::AlreadyWatched;
use crate::store::StoreManager;
use crate::{get_app_dir, SharedDirectory};
use iroh::protocol::Router;
use iroh::{Endpoint, NodeId};
use iroh_blobs::net_protocol::Blobs;
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::TopicId;
use log::{info, warn};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod actor;
pub mod protocol;
pub mod state;
pub mod fs;
pub mod gossip;
pub mod serial_state;
pub mod sync;

const DOWNLOAD_DIRNAME: &str = "download";

pub struct Engine {
    router: Router,
    gossip: Gossip,
    blobs: Blobs<iroh_blobs::store::fs::Store>,
    syncify_prot: SyncifyProtocol,
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
            .map_err(EngineError::Endpoint)?;

        // Router
        let builder = Router::builder(endpoint);

        // Blobs protocol
        let download_dir = get_app_dir().join(DOWNLOAD_DIRNAME);

        if !download_dir.exists() {
            tokio::fs::create_dir_all(&download_dir)
                .await
                .map_err(EngineError::IO)?
        }

        let blobs = Blobs::persistent(download_dir)
            .await
            .map_err(EngineError::Blobs)?
            .build(builder.endpoint());

        // Gossip protocol
        let gossip = Gossip::builder()
            .spawn(builder.endpoint().clone())
            .await
            .map_err(EngineError::Gossip)?;

        let syncify_prot = SyncifyProtocol {
            store: store.clone(),
            endpoint: builder.endpoint().clone()
        };

        let mut engine = Self {
            router: builder
                .accept(protocol::SYNCIFY_ALPN, syncify_prot.clone())
                .accept(iroh_blobs::ALPN, blobs.clone())
                .accept(iroh_gossip::ALPN, gossip.clone())
                .spawn()
                .await
                .map_err(EngineError::Router)?,
            blobs,
            gossip,
            syncify_prot,
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
            if !&dir.path.exists() {
                warn!("Directory for {} don't exist", dir.uuid);
                
                // Create the dir and its parent
                tokio::fs::create_dir_all(&dir.path)
                    .await.map_err(EngineError::IO)?
            }
            
            let topic = self.gossip.subscribe(
                TopicId::from_bytes(
                    <[u8; 32]>::try_from(dir.uuid().as_simple().to_string().as_bytes()).unwrap()
                ),
                dir.inner.read().await.neighbors.keys().map(|n| { NodeId::from_bytes(n).unwrap() }).collect(),
            ).map_err(EngineError::Gossip)?;

            // Create manager
            let manager =
                DirectoryManager::new(dir.clone(), topic, self.syncify_prot.clone()).map_err(EngineError::CannotWatch)?;

            // Store the handle in the inner
            dir.inner.write().await.handle = Some(manager.clone());

            self.managers.insert(dir.uuid, manager);
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
}

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("Filesystem error: {0}")]
    IO(io::Error),

    #[error("Endpoint error: {0}")]
    Endpoint(anyhow::Error),

    #[error("Blobs error: {0}")]
    Blobs(anyhow::Error),

    #[error("Gossip error: {0}")]
    Gossip(iroh_gossip::net::Error),

    #[error("Router error: {0}")]
    Router(anyhow::Error),

    #[error("Filesystem Watcher error (is it a network filesystem?): {0}")]
    CannotWatch(notify::Error),

    #[error("Unable to unwatch directory: {0}")]
    CannotUnwatch(notify::Error),

    #[error("Directory is already watched: {0}")]
    AlreadyWatched(Uuid),
}
