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

use crate::SyncifyError::{
    AlreadyShared, DirectoryNotEmpty, InvalidPath, NotADirectory, NotShared, PathEncoding, ReadOnly,
};
use crate::engine::job::{DownloadJob, LocalProvision, RemoteProvision};
use crate::engine::manager::ManagerHandle;
use crate::engine::state::{HashTree, State};
use crate::engine::{Engine, EngineError};
use crate::store::StoreManager;
use crate::store::link::{Link, LinkError};
use crate::store::lock::StoreLock;
use blake3::Hash;
use chacha20poly1305::aead::OsRng;
use ed25519_dalek::{SigningKey, VerifyingKey};
use iroh_base::NodeId;
use log::{info, warn};
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::HashMap;
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

#[derive(Clone)]
/// Entry point of the library.
pub struct Syncify {
    store: Arc<RwLock<StoreManager>>,
    engine: Option<Arc<RwLock<Engine>>>,
}

impl Syncify {
    /// Construct new instance.
    pub async fn new() -> Result<Self, SyncifyError> {
        let store = StoreManager::new().await.map_err(SyncifyError::Store)?;

        Ok(Self { store, engine: None })
    }

    /// Initialize new engine and start syncing.
    pub async fn start_sync(&mut self) -> Result<(), SyncifyError> {
        let engine = Engine::new(self.store.clone()).await.map_err(SyncifyError::Engine)?;
        self.engine = Some(Arc::new(RwLock::new(engine)));
        Ok(())
    }

    /// Stop syncing destroy current engine.
    pub async fn stop_sync(mut self) -> Result<Self, SyncifyError> {
        let old_engine = self.engine.take();

        if let Some(engine) = old_engine {
            engine.write().await.shutdown().await;
        }

        // Flushing store
        self.store.write().await.flush().await.map_err(SyncifyError::Store)?;

        Ok(self)
    }

    /// Create shared directory.
    pub async fn create_shared_directory(&mut self, path: PathBuf) -> Result<SharedDirectory, SyncifyError> {
        // Check and format path
        let abs_path = self.internal_check_path(path).await?;

        // Create the SharedDirectory
        let uuid = Uuid::new_v4();
        let sign_key = SigningKey::generate(&mut OsRng);
        let state = State::new(uuid);
        let tree = state.hash_tree().clone();

        let dir = SharedDirectory {
            uuid,
            path: abs_path.clone(),
            state: StoreLock::new(&self.store, state, uuid),
            local_tree: StoreLock::new(&self.store, tree, uuid),
            neighbors: StoreLock::new(&self.store, HashMap::new(), uuid),
            local_provisions: StoreLock::new(&self.store, HashMap::new(), uuid),
            remote_provisions: StoreLock::new(&self.store, HashMap::new(), uuid),
            handle: Arc::new(RwLock::new(None)),
            initial_sync: Arc::new(RwLock::new(false)),
            write_key: Some(sign_key.clone()),
            read_key: sign_key.verifying_key(),
        };

        info!(
            "Creating shared directory {} at \"{}\"",
            dir.uuid(),
            dir.path().display()
        );

        self.internal_add_directory(&dir).await?;

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
                    .write()
                    .await
                    .remove_watched_directory(&dir)
                    .await
                    .map_err(SyncifyError::Engine)?;
            }

            Ok(())
        } else {
            Err(NotShared)
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
        // Check and format path
        let abs_path = self.internal_check_path(path).await?;

        // Create the SharedDirectory
        let sign_key = if link.permission == SharedDirPermission::Write {
            Some(SigningKey::from_bytes(&link.key))
        } else {
            None
        };

        let state = State::new(link.uuid);
        let tree = state.hash_tree().clone();

        // Neighbors from link
        let mut neighbors = HashMap::new();
        for n in link.neighbors {
            if let Ok(node_id) = NodeId::from_bytes(&n) {
                neighbors.insert(node_id, false);
            }
        }

        let dir = SharedDirectory {
            uuid: link.uuid,
            path: abs_path.clone(),
            state: StoreLock::new(&self.store, state, link.uuid),
            local_tree: StoreLock::new(&self.store, tree, link.uuid),
            neighbors: StoreLock::new(&self.store, neighbors, link.uuid),
            local_provisions: StoreLock::new(&self.store, HashMap::new(), link.uuid),
            remote_provisions: StoreLock::new(&self.store, HashMap::new(), link.uuid),
            handle: Arc::new(RwLock::new(None)),
            initial_sync: Arc::new(RwLock::new(false)),
            write_key: sign_key.clone(),
            read_key: if let Some(key) = sign_key {
                key.verifying_key()
            } else {
                VerifyingKey::from_bytes(&link.key).unwrap()
            },
        };

        info!("Added shared directory {} at \"{}\"", dir.uuid(), dir.path().display());

        self.internal_add_directory(&dir).await?;

        Ok(dir)
    }

    /// Check the [`PathBuf`], and create directory if necessary and return formatted version.
    async fn internal_check_path(&mut self, path: PathBuf) -> Result<PathBuf, SyncifyError> {
        // Ge the absolute version
        let mut abs_path = std::path::absolute(&path).map_err(InvalidPath)?;

        // Add a trailing "/" at the end of the path
        abs_path.push("");

        // Check if the dir exists
        match abs_path.try_exists() {
            Ok(exists) => {
                if !exists {
                    // Check if abs_path is a directory
                    if !abs_path.is_dir() {
                        return Err(NotADirectory);
                    }
                    // Check if the directory is empty
                    if abs_path.read_dir().iter().nth(1).is_some() {
                        return Err(DirectoryNotEmpty);
                    }
                    // Check if the user has write access in the directory
                    let md = abs_path.metadata().map_err(InvalidPath)?;
                    if md.permissions().readonly() {
                        return Err(ReadOnly);
                    }
                } else {
                    // Create the dir and its parent
                    tokio::fs::create_dir_all(&get_app_config_dir())
                        .await
                        .map_err(|e| match e.kind() {
                            ErrorKind::PermissionDenied => ReadOnly,
                            _ => InvalidPath(e),
                        })?
                }

                if abs_path.to_str().is_none() {
                    return Err(PathEncoding(abs_path));
                }

                // Verify if it already exists
                if self
                    .store
                    .read()
                    .await
                    .get_all_dirs()
                    .iter()
                    .any(|d| d.path == abs_path)
                {
                    return Err(AlreadyShared);
                }

                Ok(abs_path)
            }
            Err(e) => Err(InvalidPath(e)),
        }
    }

    /// Add the [`SharedDirectory`] to the store and create the database entries.
    async fn internal_add_directory(&mut self, dir: &SharedDirectory) -> Result<(), SyncifyError> {
        self.store
            .write()
            .await
            .add_shared_dir(dir)
            .await
            .map_err(SyncifyError::Store)?;

        if let Err(e) = dir.state.write().flush().await {
            warn!("Unable to save new state: {}", e);
        };

        if let Err(e) = dir.local_tree.write().flush().await {
            warn!("Unable to save new tree: {}", e);
        };

        if let Err(e) = dir.neighbors.write().flush().await {
            warn!("Unable to save new neighbors: {}", e);
        };

        // If the engine is available, add the directory to watched directory
        if let Some(engine) = &mut self.engine {
            engine
                .write()
                .await
                .add_watched_directory(self.store.clone(), dir)
                .await
                .map_err(SyncifyError::Engine)?;
        }

        Ok(())
    }
}

pub type NeighborsMap = HashMap<NodeId, bool>;
pub type LocalProvisionsMap = HashMap<Hash, LocalProvision>;
pub type RemoteProvisionsMap = HashMap<Hash, HashMap<NodeId, RemoteProvision>>;
pub type DownloadJobsMap = HashMap<Hash, Arc<RwLock<DownloadJob>>>;
pub type ActiveDownloadJobs = Vec<Arc<RwLock<DownloadJob>>>;

#[derive(Clone)]
pub struct SharedDirectory {
    uuid: Uuid,
    path: PathBuf,
    state: StoreLock<State>,
    local_tree: StoreLock<HashTree>,
    neighbors: StoreLock<NeighborsMap>,
    local_provisions: StoreLock<LocalProvisionsMap>,
    remote_provisions: StoreLock<RemoteProvisionsMap>,
    handle: Arc<RwLock<Option<ManagerHandle>>>,
    initial_sync: Arc<RwLock<bool>>,
    write_key: Option<SigningKey>,
    read_key: VerifyingKey,
}

impl SharedDirectory {
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    pub fn name(&self) -> String {
        self.path.file_name().unwrap().to_string_lossy().into_owned()
    }

    pub fn is_read_only(&self) -> bool {
        self.write_key.is_none()
    }

    /// Get the handle. Panics if not available.
    pub(crate) async fn handle(&self) -> ManagerHandle {
        if let Some(handle) = self.handle.read().await.as_ref() {
            handle.clone()
        } else {
            panic!("Handle is not available for {}", self.uuid);
        }
    }
}

#[derive(Error, Debug)]
pub enum SyncifyError {
    #[error("{0}")]
    Store(store::StoreError),

    #[error("{0}")]
    Engine(EngineError),

    #[error("{0}")]
    LinkError(LinkError),

    #[error("Engine not initialized")]
    EngineNotInit(),

    #[error("Invalid path: {0}")]
    InvalidPath(std::io::Error),

    #[error("Invalid path (not UTF-8): {0}")]
    PathEncoding(PathBuf),

    #[error("Directory is is not writable")]
    ReadOnly,

    #[error("Directory is already shared")]
    AlreadyShared,

    #[error("Directory is not shared")]
    NotShared,

    #[error("Directory is not empty")]
    DirectoryNotEmpty,

    #[error("Not a directory")]
    NotADirectory,
}
