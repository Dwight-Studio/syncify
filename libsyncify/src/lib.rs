use crate::engine::state::State;
use crate::engine::{Engine, EngineError};
use crate::store::StoreManager;
use crate::SyncifyError::{AlreadyShared, InvalidPath, NotShared, ReadOnly};
use chacha20poly1305::aead::OsRng;
use ed25519_dalek::{SigningKey, VerifyingKey};
use log::info;
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::PartialEq;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod engine;
pub mod store;
pub mod util;

// Set the path where the store file will be/is stored
fn get_app_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("target/debug/cache")
    } else {
        let project_dir = directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
        project_dir.config_local_dir().to_path_buf()
    }
}

#[derive(PartialEq, Clone)]
#[derive(Archive, Serialize, Deserialize)]
pub enum SharedDirPermission {
    ReadOnly,
    Write,
}

const APP_NAME: &str = "Syncify";

/// Entry point of the library.
pub struct Syncify {
    store: Arc<RwLock<StoreManager>>,
    engine: Option<Engine>,
}

impl Syncify {
    /// Construct new instance.
    pub async fn new() -> Result<Self, SyncifyError> {
        let store = StoreManager::new().await.map_err(SyncifyError::Store)?;

        Ok(Self {
            store: Arc::new(RwLock::new(store)),
            engine: None,
        })
    }

    /// Initialize new engine and start syncing.
    pub async fn start_sync(&mut self) -> Result<(), SyncifyError> {
        self.engine = Some(
            Engine::new(self.store.clone())
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

        // Flushing store
        self.store.write().await.flush().await;

        self
    }

    /// Create shared directory.
    pub async fn create_shared_directory(
        &mut self,
        path: PathBuf,
    ) -> Result<SharedDirectory, SyncifyError> {
        let abs_path = std::path::absolute(&path).map_err(InvalidPath)?;

        // Check if the directory is already shared
        for dir in &self.store.read().await.get_all_dirs() {
            if dir.path == abs_path {
                return Err(AlreadyShared(abs_path.clone()));
            }
        }

        // Check if the dir exists
        if fs::exists(&abs_path).map_err(InvalidPath)? {
            // Check if the user has write access in the directory
            let md = fs::metadata(abs_path.clone()).map_err(InvalidPath)?;
            if md.permissions().readonly() {
                return Err(ReadOnly(abs_path));
            }
        } else {
            // Create the dir and its parent
            tokio::fs::create_dir_all(&get_app_dir())
                .await.map_err(|e| match e.kind() {
                ErrorKind::PermissionDenied => ReadOnly(abs_path.clone()),
                _ => InvalidPath(e)
            })?
        }

        // Add the directory to the store
        let uuid = Uuid::new_v4();
        let sign_key = SigningKey::generate(&mut OsRng);

        let dir = SharedDirectory {
            uuid,
            path: abs_path.clone(),
            state: Arc::new(RwLock::new(State::new(abs_path.file_name().unwrap().to_string_lossy().to_string()))),
            sign_key: Some(sign_key.clone()),
            verif_key: sign_key.verifying_key(),
        };

        info!(
            "Creating shared directory {} at \"{}\"",
            dir.uuid(),
            dir.path().display()
        );

        self.store.write().await.add_shared_dir(&dir).await.map_err(SyncifyError::StoreKeyring)?;

        // If the engine is available, add the directory to watched directory
        if let Some(engine) = &mut self.engine {
            engine
                .add_watched_directory(self.store.clone(), &dir)
                .await
                .map_err(SyncifyError::Watcher)?;
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
        if self.store.read().await.get_shared_dir(&dir.uuid).is_some() {
            self.store.write().await.remove_shared_dir(&dir).await.map_err(SyncifyError::StoreKeyring)?;

            // If the engine is available, add the directory to watched directory
            if let Some(engine) = &mut self.engine {
                engine
                    .remove_watched_directory(&dir)
                    .await
                    .map_err(SyncifyError::Watcher)?;
            }

            Ok(())
        } else {
            Err(NotShared(dir.path()))
        }
    }

    /// Get an existing shared directory.
    pub async fn get_shared_directory(&self, uuid: &Uuid) -> Option<SharedDirectory> {
        self.store.read().await.get_shared_dir(uuid)
    }

    /// Get all existing shared directories
    pub async fn get_all_shared_directories(&self) -> Vec<SharedDirectory> {
        self.store.read().await.get_all_dirs()
    }

    pub async fn build_link(
        &self,
        uuid: Uuid,
        permission: SharedDirPermission,
    ) -> Result<String, SyncifyError> {
        let dir = self.get_shared_directory(&uuid).await.unwrap();
        let key = {
            if permission == SharedDirPermission::Write {
                if let Some(tmp) = dir.sign_key {
                    String::from_utf8_lossy(tmp.as_bytes()).to_string()
                } else {
                    return Err(SyncifyError::DirectoryReadOnly());
                }
            } else {
                String::from_utf8_lossy(dir.verif_key.as_bytes()).to_string()
            }
        };

        //let link = format!("syncify://?key={}&payload={}", key);

        Ok(String::new())
    }
}

#[derive(Clone)]
pub struct SharedDirectory {
    uuid: Uuid,
    path: PathBuf,
    state: Arc<RwLock<State>>,
    sign_key: Option<SigningKey>,
    verif_key: VerifyingKey,
}

impl SharedDirectory {
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
    
    pub fn is_read_only(&self) -> bool {
        self.sign_key.is_none()
    }
}

#[derive(Error, Debug)]
pub enum SyncifyError {
    #[error("Store error: {0}")]
    Store(store::StoreError),

    #[error("Store error: {0}")]
    StoreKeyring(keyring::Error),

    #[error("Engine {0}")]
    Engine(EngineError),

    #[error("Engine not initialized")]
    EngineNotInit(),

    #[error("Invalid path: {0}")]
    InvalidPath(std::io::Error),

    #[error("Directory is is not writable: {0}")]
    ReadOnly(PathBuf),

    #[error("Directory is already shared: {0}")]
    AlreadyShared(PathBuf),

    #[error("Directory is not shared")]
    NotShared(PathBuf),

    #[error("Shared directory is in read-only mode")]
    DirectoryReadOnly(),

    #[error("{0}")]
    Watcher(EngineError),

    #[error("Shared directory does not exists: {0}")]
    DirectoryDoesNotExists(Uuid),
    
    #[error("Error while parsing link: {0}")]
    LinkParseError(String),
}
