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

use crate::SyncifyError::{AlreadyShared, DirectoryNotEmpty, InvalidPath, NotADirectory, NotShared, ReadOnly};
use crate::engine::manager::ManagerHandle;
use crate::engine::state::State;
use crate::engine::{Engine, EngineError};
use crate::store::StoreManager;
use crate::store::link::Link;
use blake3::Hash;
use chacha20poly1305::aead::OsRng;
use ed25519_dalek::{SigningKey, VerifyingKey};
use log::info;
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use iroh_base::NodeId;
use thiserror::Error;
use tokio::sync::RwLockReadGuard;
use tokio::sync::{RwLock, RwLockWriteGuard};
use uuid::Uuid;
use crate::engine::downloader::Provision;

pub mod engine;
pub mod store;
pub mod util;

// Set the path where the store file will be/is stored
fn get_app_config_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("target/debug/cache")
    } else {
        let project_dir = directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
        project_dir.config_local_dir().to_path_buf()
    }
}

fn get_app_cache_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from("target/debug/cache/cache")
    } else {
        let project_dir = directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
        project_dir.cache_dir().to_path_buf()
    }
}

#[derive(PartialEq, Clone, Archive, Serialize, Deserialize)]
pub enum SharedDirPermission {
    ReadOnly,
    Write,
}

pub const APP_NAME: &str = "Syncify";

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
        self.engine = Some(Engine::new(self.store.clone()).await.map_err(SyncifyError::Engine)?);
        Ok(())
    }

    /// Stop syncing destroy current engine.
    pub async fn stop_sync(mut self) -> Result<Self, SyncifyError> {
        let old_engine = self.engine.take();

        if let Some(engine) = old_engine {
            engine.shutdown().await;
        }

        // Flushing store
        self.store.write().await.flush().await.map_err(SyncifyError::Store)?;

        Ok(self)
    }

    /// Create shared directory.
    pub async fn create_shared_directory(&mut self, path: PathBuf) -> Result<SharedDirectory, SyncifyError> {
        let mut abs_path = std::path::absolute(&path).map_err(InvalidPath)?;

        // Add a trailing "/" at the end of the path
        abs_path.push("");

        // Check if the directory is already shared
        for dir in &self.store.read().await.get_all_dirs() {
            if dir.path == abs_path {
                return Err(AlreadyShared(abs_path.clone()));
            }
        }

        // Check if the dir exists
        if abs_path.exists() {
            // Check if the user has write access in the directory
            if !abs_path.is_dir() {
                return Err(NotADirectory(abs_path));
            }
            let md = abs_path.metadata().map_err(InvalidPath)?;
            if md.permissions().readonly() {
                return Err(ReadOnly(abs_path));
            }
        } else {
            // Create the dir and its parent
            tokio::fs::create_dir_all(&get_app_config_dir())
                .await
                .map_err(|e| match e.kind() {
                    ErrorKind::PermissionDenied => ReadOnly(abs_path.clone()),
                    _ => InvalidPath(e),
                })?
        }

        // Add the directory to the store
        let uuid = Uuid::new_v4();
        let sign_key = SigningKey::generate(&mut OsRng);
        let state = State::new(uuid);

        let dir = SharedDirectory {
            uuid,
            path: abs_path.clone(),
            inner: Arc::new(RwLock::new(InnerSharedDirectory::new(
                state,
                HashMap::new(),
            ))),
            sign_key: Some(sign_key.clone()),
            verif_key: sign_key.verifying_key(),
        };

        info!(
            "Creating shared directory {} at \"{}\"",
            dir.uuid(),
            dir.path().display()
        );

        self.store
            .write()
            .await
            .add_shared_dir(&dir)
            .await
            .map_err(SyncifyError::Store)?;

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
    pub async fn remove_shared_directory(&mut self, dir: SharedDirectory) -> Result<(), SyncifyError> {
        if self.store.read().await.get_shared_dir(&dir.uuid).is_some() {
            self.store
                .write()
                .await
                .remove_shared_dir(&dir)
                .await
                .map_err(SyncifyError::Store)?;

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

    /// Join a shared directory
    pub async fn join_shared_directory(&mut self, link: Link, path: PathBuf) -> Result<SharedDirectory, SyncifyError> {
        let mut abs_path = std::path::absolute(&path).map_err(InvalidPath)?;

        // Add a trailing "/" at the end of the path
        abs_path.push("");

        // Check if the dir exists
        if abs_path.exists() {
            // Check if abs_path is a directory
            if !abs_path.is_dir() {
                return Err(NotADirectory(abs_path));
            }
            // Check if the directory is empty
            if abs_path.read_dir().iter().nth(1).is_some() {
                return Err(DirectoryNotEmpty(abs_path));
            }
            // Check if the user has write access in the directory
            let md = abs_path.metadata().map_err(InvalidPath)?;
            if md.permissions().readonly() {
                return Err(ReadOnly(abs_path));
            }
        } else {
            // Create the dir and its parent
            tokio::fs::create_dir_all(&get_app_config_dir())
                .await
                .map_err(|e| match e.kind() {
                    ErrorKind::PermissionDenied => ReadOnly(abs_path.clone()),
                    _ => InvalidPath(e),
                })?
        }

        // Add the directory to the store
        let sign_key = if link.permission == SharedDirPermission::Write {
            Some(SigningKey::from_bytes(&link.key))
        } else {
            None
        };
        let state = State::new(link.uuid);

        let dir = SharedDirectory {
            uuid: link.uuid,
            path: abs_path.clone(),
            inner: Arc::new(RwLock::new(InnerSharedDirectory::new(
                state,
                link.neighbors,
            ))),
            sign_key: sign_key.clone(),
            verif_key: if let Some(key) = sign_key {
                key.verifying_key()
            } else {
                VerifyingKey::from_bytes(&link.key).unwrap()
            },
        };

        info!("Added shared directory {} at \"{}\"", dir.uuid(), dir.path().display());

        self.store
            .write()
            .await
            .add_shared_dir(&dir)
            .await
            .map_err(SyncifyError::Store)?;

        // If the engine is available, add the directory to watched directory
        if let Some(engine) = &mut self.engine {
            engine
                .add_watched_directory(self.store.clone(), &dir)
                .await
                .map_err(SyncifyError::Watcher)?;
        }

        Ok(dir)
    }
}

#[derive(Clone)]
pub struct SharedDirectory {
    uuid: Uuid,
    path: PathBuf,
    inner: Arc<RwLock<InnerSharedDirectory>>,
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

    pub(crate) async fn read(&self) -> RwLockReadGuard<InnerSharedDirectory> {
        self.inner.read().await
    }

    pub(crate) async fn write(&self) -> RwLockWriteGuard<InnerSharedDirectory> {
        self.inner.write().await
    }
    
    /// Get the handle. Panics if not available.
    pub(crate) async fn handle(&self) -> ManagerHandle {
        if let Some(handle) = &self.read().await.handle {
            handle.clone()
        } else {
            panic!("Handle is not available for {}", self.uuid);
        }
    }
}

pub(crate) struct InnerSharedDirectory {
    pub(crate) state: State,
    pub(crate) neighbors: HashMap<[u8; 32], bool>,
    pub(crate) provisions: HashMap<Hash, HashMap<NodeId, DateTime<Utc>>>,
    pub(crate) handle: Option<ManagerHandle>,
    pub(crate) received_initial_sync: bool,
}

impl InnerSharedDirectory {
    pub(crate) fn new(state: State, neighbors: HashMap<[u8; 32], bool>) -> Self {
        Self {
            state,
            neighbors,
            provisions: HashMap::new(),
            handle: None,
            received_initial_sync: false,
        }
    }
}

#[derive(Error, Debug)]
pub enum SyncifyError {
    #[error("Store error: {0}")]
    Store(store::StoreError),

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

    #[error("Directory is not empty: {0}")]
    DirectoryNotEmpty(PathBuf),

    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
}
