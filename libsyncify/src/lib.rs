use crate::config::{SyncifyConfig, SyncifyFolder};
use std::path::{PathBuf};
use thiserror::Error;
use uuid::Uuid;
use crate::engine::{Engine, EngineError};
use crate::SyncifyError::InvalidPath;

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
    /// Create new instance.
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
        let old_engine = self.engine;
        self.engine = None;
        
        if let Some(engine) = old_engine {
            engine.destroy().await;
        }
        
        self
    }
    
    /// Create shared directory.
    pub async fn create_shared_directory(&mut self, path: PathBuf) -> Result<Uuid, SyncifyError> {
        if !path.exists() {
            return Err(InvalidPath(path))
        }
        
        // TODO: Check if the shared directory already exists
        
        if let Some(engine) = &mut self.engine {
            let uuid = Uuid::new_v4();
            
            let folder = SyncifyFolder {
                uuid: uuid.clone(),
                path: path.to_string_lossy().to_string()
            };

            engine.add_watched_directory(&folder)
                .await
                .map_err(SyncifyError::CannotWatch)?;
            
            self.config.config_data.folders.push(folder);
            
            Ok(uuid)
        } else {
            Err(SyncifyError::EngineNotInit())
        }
    }
}

#[derive(Error, Debug)]
pub enum SyncifyError {
    #[error("Config error: {0}")]
    Config(std::io::Error),

    #[error("Engine {0}")]
    Engine(engine::EngineError),
    
    #[error("Engine not initialized")]
    EngineNotInit(),
    
    #[error("Invalid path: {0}")]
    InvalidPath(PathBuf),

    #[error("{0}")]
    CannotWatch(EngineError)
}
