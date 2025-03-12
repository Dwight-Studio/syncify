use crate::engine::{Engine, EngineError};
use crate::store::{SharedDirectoryData, StoreManager};
use crate::SyncifyError::{AlreadyShared, InvalidPath, NotShared};
use chacha20poly1305::aead::{Key, OsRng};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use iroh::Endpoint;
use std::path::{PathBuf};
use std::sync::Arc;
use base64::Engine as Base64Engine;
use base64::prelude::BASE64_STANDARD;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

mod engine;
mod store;

// Set the path where the configs file will be/is stored
fn get_app_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("target/debug/cache")
    } else {
        let project_dir = directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
        project_dir.config_local_dir().to_path_buf()
    }
}

const APP_NAME: &str = "Syncify";

/// Entry point of the library.
pub struct Syncify {
    config: Arc<RwLock<StoreManager>>,
    engine: Option<Engine>,
}

impl Syncify {
    /// Construct new instance.
    pub async fn new() -> Result<Self, SyncifyError> {
        let config = StoreManager::new().await.map_err(SyncifyError::Config)?;

        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            engine: None,
        })
    }

    /// Initialize new engine and start syncing.
    pub async fn start_sync(&mut self) -> Result<(), SyncifyError> {
        self.engine = Some(
            Engine::new(self.config.clone())
                .await
                .map_err(SyncifyError::Engine)?,
        );
        Ok(())
    }

    /// Stop syncing destroy current engine.
    pub async fn stop_sync(mut self) -> Self {
        let old_engine = self.engine.take();

        if let Some(engine) = old_engine {
            engine.shutdown().await;
        }

        self
    }

    /// Create shared directory.
    pub async fn create_shared_directory(
        &mut self,
        path: PathBuf,
    ) -> Result<SharedDirectory, SyncifyError> {
        let canonical_path = std::fs::canonicalize(&path).map_err(|_| InvalidPath(path))?;

        // Check if the directory is already shared
        for folder in &self.config.read().await.data.shared_directories {
            if PathBuf::from(&folder.path).eq(&canonical_path) {
                return Err(AlreadyShared(canonical_path));
            }
        }

        // Add the directory to the store
        let uuid = Uuid::new_v4();

        let dir = SharedDirectory {
            data: SharedDirectoryData {
                uuid,
                path: canonical_path.to_string_lossy().to_string(),
            },
            key: XChaCha20Poly1305::generate_key(&mut OsRng),
        };

        self.config.write().await.add_shared_dir(&dir);

        // If the engine is available, add the directory to watched directory
        if let Some(engine) = &mut self.engine {
            engine
                .add_watched_directory(&dir.path())
                .await
                .map_err(SyncifyError::CannotWatch)?;
        }

        Ok(dir)
    }

    /// Remove shared directory.
    ///
    /// No files are actually deleted, but the directory will no longer be synchronized.
    pub async fn remove_shared_directory(
        &mut self,
        dir: SharedDirectory,
    ) -> Result<(), SyncifyError> {
        if self
            .config
            .read()
            .await
            .data
            .shared_directories
            .contains(&dir.data)
        {
            self.config.write().await.remove_shared_dir(&dir);

            // If the engine is available, add the directory to watched directory
            if let Some(engine) = &mut self.engine {
                engine
                    .remove_watched_directory(&dir.path())
                    .await
                    .map_err(SyncifyError::CannotWatch)?;
            }

            Ok(())
        } else {
            Err(NotShared(dir.data))
        }
    }

    /// Get an existing shared directory.
    pub async fn get_shared_directory(&self, uuid: &Uuid) -> Option<SharedDirectory> {
        self.config.read().await.get_shared_dir(uuid)
    }

    #[cfg(debug_assertions)]
    pub fn get_node_endpoint(&self) -> &Endpoint {
        self.engine.as_ref().unwrap().get_node_endpoint()
    }
}

pub struct SharedDirectory {
    data: SharedDirectoryData,
    key: Key<XChaCha20Poly1305>,
}

impl SharedDirectory {
    pub fn uuid(&self) -> Uuid {
        self.data.uuid
    }

    pub fn path(&self) -> PathBuf {
        PathBuf::from(&self.data.path)
    }
    
    pub fn key(&self) -> String {
        BASE64_STANDARD.encode(self.key)
    }
}

#[derive(Error, Debug)]
pub enum SyncifyError {
    #[error("Config error: {0}")]
    Config(std::io::Error),

    #[error("Engine {0}")]
    Engine(EngineError),

    #[error("Engine not initialized")]
    EngineNotInit(),

    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),

    #[error("Folder is already shared: {0}")]
    AlreadyShared(PathBuf),

    #[error("Folder is not shared")]
    NotShared(SharedDirectoryData),

    #[error("{0}")]
    CannotWatch(EngineError),
}
