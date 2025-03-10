use crate::config::{SyncifyConfig, SyncifyFolder};
use std::path::PathBuf;
use thiserror::Error;
use crate::engine::Engine;

mod config;
mod engine;

// Set the path where the configs file will be/is stored
fn get_app_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("./target/debug")
    } else {
    let project_dir =
        directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
        project_dir.config_local_dir().to_path_buf()
    }
}

const APP_NAME: &str = "Syncify";

pub struct Syncify {
    pub(crate) config: SyncifyConfig,
    pub(crate) engine: Option<Engine>
}

impl Syncify {
    pub async fn new() -> Result<Self, SyncifyError> {
        let config = SyncifyConfig::new()
            .await
            .map_err(|e| SyncifyError::Config(e))?;

        //println!("{}", config);

        Ok(Self {config, engine: None })
    }

    pub async fn start_sync(&mut self) -> Result<(), SyncifyError> {
        self.engine = Some(Engine::new(&self.config)
            .await
            .map_err(|e| SyncifyError::Engine(e))?);
        Ok(())
    }

    pub async fn stop_sync(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            engine.destroy().await;
        }
    }

    pub async fn create_shared_directory(&mut self, path: PathBuf) {
        let uuid = uuid::Uuid::new_v4();
        self.config.config_data.paths.push(SyncifyFolder { uuid: uuid.to_string(), path: path.to_string_lossy().to_string() });
    }
}

#[derive(Error, Debug)]
pub enum SyncifyError {
    #[error("Config error: {0}")]
    Config(std::io::Error),

    #[error("Engine {0}")]
    Engine(engine::EngineError)
}
