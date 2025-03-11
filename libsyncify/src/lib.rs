use std::fmt::Display;
use crate::config::SyncifyConfig;
use std::path::{PathBuf};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use crate::engine::{Engine, EngineError};
use crate::SyncifyError::{AlreadyShared, InvalidPath, NotShared};

mod config;
mod engine;

// Set the path where the configs file will be/is stored
fn get_app_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("target/debug/cache")
    } else {
    let project_dir =
        directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
        project_dir.config_local_dir().to_path_buf()
    }
}

const APP_NAME: &str = "Syncify";

/// Entry point of the library.
pub struct Syncify {
    pub(crate) config: SyncifyConfig,
    pub(crate) engine: Option<Engine>
}

impl Syncify {
    /// Construct new instance.
    pub async fn new() -> Result<Self, SyncifyError> {
        let config = SyncifyConfig::new()
            .await
            .map_err(SyncifyError::Config)?;

        Ok(Self {config, engine: None })
    }

    /// Initialize new engine and start syncing.
    pub async fn start_sync(&mut self) -> Result<(), SyncifyError> {
        self.engine = Some(Engine::new(&self.config)
            .await
            .map_err(SyncifyError::Engine)?);
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
    pub async fn create_shared_directory(&mut self, path: PathBuf) -> Result<SharedDirectory, SyncifyError> {
        let canonical_path = std::fs::canonicalize(&path).map_err(|_| InvalidPath(path))?;

        // Check if the directory is already shared
        for folder in &self.config.data.dirs {
            if PathBuf::from(&folder.path).eq(&canonical_path) {
                return Err(AlreadyShared(canonical_path));
            }
        }

        // Add the directory to the config
        let uuid = Uuid::new_v4();

        let dir = SharedDirectory {
            uuid,
            path: canonical_path.to_string_lossy().to_string()
        };

        self.config.data.dirs.push(dir.clone());

        // If the engine is available, add the directory to watched directory
        if let Some(engine) = &mut self.engine {
            engine.add_watched_directory(&dir)
                .await
                .map_err(SyncifyError::CannotWatch)?;
        }

        Ok(dir)
    }

    /// Remove shared directory.
    ///
    /// No files are actually deleted, but the directory will no longer be synchronized.
    pub async fn remove_shared_directory(&mut self, dir: SharedDirectory) -> Result<(), SyncifyError> {
        if self.config.data.dirs.contains(&dir) {
            self.config.data.dirs.retain(|shared_directory| !dir.uuid.eq(&shared_directory.uuid));

            // If the engine is available, add the directory to watched directory
            if let Some(engine) = &mut self.engine {
                engine.remove_watched_directory(&dir)
                    .await
                    .map_err(SyncifyError::CannotWatch)?;
            }

            Ok(())
        } else {
            Err(NotShared(dir))
        }
    }

    /// Get an existing shared directory.
    pub fn get_shared_directory(&self, uuid: &Uuid) -> Option<SharedDirectory> {
        self.config.data.dirs
            .iter()
            .find(|folder| folder.uuid == *uuid)
            .cloned()
    }
}

/// Shared directory.
#[derive(Serialize, Deserialize, Debug)]
pub struct SharedDirectory {
    #[serde(with = "config::UuidDef")]
    pub(crate) uuid: Uuid,
    pub(crate) path: String,
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
    NotShared(SharedDirectory),

    #[error("{0}")]
    CannotWatch(EngineError)
}
