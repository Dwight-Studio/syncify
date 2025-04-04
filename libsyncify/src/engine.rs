/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use crate::SharedDirectory;
use crate::engine::EngineError::AlreadyWatched;
use crate::engine::downloader::Downloader;
use crate::engine::manager::Manager;
use crate::engine::protocol::{SYNCIFY_ALPN, SyncifyProtocol, SyncifyProtocolHandler};
use crate::store::StoreManager;
use iroh::Endpoint;
use iroh::protocol::Router;
use iroh_gossip::net::Gossip;
use iroh_gossip::proto::TopicId;
use log::{error, info, warn};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod downloader;
pub mod job;
pub mod manager;
pub mod protocol;
pub mod state;

/// Auto flush period.
pub const AUTO_FLUSH_PERIOD: Duration = Duration::from_secs(30 * 60);

pub struct Engine {
    _store: Arc<RwLock<StoreManager>>,
    router: Router,
    gossip: Gossip,
    ep: Endpoint,
    downloader: Downloader,
    managers: HashMap<Uuid, Manager>,
    proto: Arc<RwLock<SyncifyProtocol>>,
}

/// Synchronization engine.
impl Engine {
    /// Construct new instance.
    pub async fn new(store: Arc<RwLock<StoreManager>>) -> Result<Self, EngineError> {
        info!("Initializing engine");
        let endpoint = Endpoint::builder()
            .secret_key(store.read().await.secret_key())
            .alpns(vec![iroh_gossip::ALPN.to_vec(), SYNCIFY_ALPN.to_vec()])
            .discovery_n0()
            .discovery_local_network()
            .bind()
            .await
            .map_err(EngineError::Endpoint)?;

        // Router
        let builder = Router::builder(endpoint);

        // Gossip protocol
        let gossip = Gossip::builder()
            .spawn(builder.endpoint().clone())
            .await
            .map_err(EngineError::Gossip)?;

        let protocol = Arc::new(RwLock::new(SyncifyProtocol::new(
            builder.endpoint().clone(),
            store.clone(),
        )));
        let downloader = Downloader::new(store.clone(), protocol.clone());
        protocol.write().await.set_downloader(downloader.clone());
        let protocol_handler = SyncifyProtocolHandler::new(protocol.clone(), store.clone(), downloader.clone());

        let mut engine = Self {
            _store: store.clone(),
            ep: builder.endpoint().clone(),
            router: builder
                .accept(SYNCIFY_ALPN, protocol_handler.clone())
                .accept(iroh_gossip::ALPN, gossip.clone())
                .spawn()
                .await
                .map_err(EngineError::Router)?,
            gossip,
            downloader,
            managers: HashMap::new(),
            proto: protocol,
        };

        for dir in &store.read().await.get_all_dirs() {
            engine.add_watched_directory(store.clone(), dir).await?
        }

        Ok(engine)
    }

    /// Gracefully shutdown.
    pub async fn shutdown(&mut self) {
        info!("Shutting down engine");
        self.router.shutdown().await.unwrap();
        for ref mut entries in &mut self.managers {
            let (_, manager) = entries;
            manager.shutdown().await;
        }
        self.downloader.shutdown().await;
    }

    /// Create [`Manager`] manager for a [`SharedDirectory`].
    pub async fn add_watched_directory(
        &mut self,
        _store: Arc<RwLock<StoreManager>>,
        dir: &SharedDirectory,
    ) -> Result<(), EngineError> {
        if !self.managers.contains_key(&dir.uuid()) {
            match dir.path.try_exists() {
                Ok(exists) => {
                    if !exists {
                        warn!("Directory for {} don't exist", dir.uuid);

                        // Create the dir and its parent
                        tokio::fs::create_dir_all(&dir.path).await.map_err(EngineError::IO)?
                    }
                }

                Err(e) => return Err(EngineError::IO(e)),
            }

            let topic = self
                .gossip
                .subscribe(
                    TopicId::from_bytes(<[u8; 32]>::try_from(dir.uuid().as_simple().to_string().as_bytes()).unwrap()),
                    dir.neighbors.read().await.iter().map(|(node_id, _)| *node_id).collect(),
                )
                .map_err(EngineError::Gossip)?;

            // Create manager
            let manager = Manager::new(
                dir.clone(),
                topic,
                self.ep.clone(),
                self.downloader.clone(),
                self.proto.clone(),
            )
            .await;

            self.managers.insert(dir.uuid, manager);
            Ok(())
        } else {
            Err(AlreadyWatched(dir.uuid()))
        }
    }

    pub async fn remove_watched_directory(&mut self, dir: &SharedDirectory) -> Result<(), EngineError> {
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

    #[error("Directory is already watched: {0}")]
    AlreadyWatched(Uuid),
}
