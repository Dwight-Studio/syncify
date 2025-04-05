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
use crate::engine::job::{DownloadJob, LocalProvision, RemoteProvision};
use crate::engine::state::{Delta, HashTree, State};
use crate::store::keyring::{Keyring, Keys};
use crate::store::lock::StoreLock;
use crate::{LocalProvisionsMap, ReadKey, RemoteProvisionsMap, SharedDirectory, WriteKey, get_app_config_dir};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use blake3::Hash;
use chacha20poly1305::aead::OsRng;
use iroh::SecretKey;
use iroh_base::{NodeId, PublicKey};
use log::{error, info, warn};
use redb::{
    CommitError, Database, DatabaseError, MultimapTableDefinition, MultimapValue, ReadableMultimapTable, ReadableTable,
    StorageError, TableDefinition, TableError, TableHandle, TransactionError, WriteTransaction,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod keyring;
pub mod link;
pub mod lock;

/// Store file name.
pub const STORE_FILENAME: &str = "store.db";

// Database tables

/// Base table (Directory path -> UUID).
pub const BASE_TABLE: TableDefinition<&str, [u8; 16]> = TableDefinition::new("base");
/// Head table (UUID -> State head hash).
pub const HEAD_TABLE: TableDefinition<[u8; 16], [u8; 32]> = TableDefinition::new("head");
/// Local tree table (UUID -> Local file tree)
pub const LOCAL_TREE_TABLE: TableDefinition<[u8; 16], HashTree> = TableDefinition::new("local_tree");
/// Neighbors table (UUID -> Vec of all neighbors NodeIDs).
pub const NEIGHBORS_TABLE: TableDefinition<[u8; 16], Vec<[u8; 32]>> = TableDefinition::new("neighbors");
/// Local provisions table (UUID -> * Provision), describing the files that are provided to other pairs.
pub const LOCAL_PROVISIONS_TABLE: MultimapTableDefinition<[u8; 16], LocalProvision> =
    MultimapTableDefinition::new("local-provision");
/// Remote provisions table (UUID -> * Provision), describing the files that are provided by other pairs.
pub const REMOTE_PROVISIONS_TABLE: MultimapTableDefinition<[u8; 16], RemoteProvision> =
    MultimapTableDefinition::new("remote-provision");
/// Jobs table (Hash of the file -> DownloadJob).
pub const JOBS_TABLE: TableDefinition<[u8; 32], DownloadJob> = TableDefinition::new("jobs");

/// Store manager.
pub struct StoreManager {
    db: Database,
    cache: HashMap<Uuid, SharedDirectory>,
    secret_key: SecretKey,
    keyring: Keyring,
}

impl StoreManager {
    pub async fn new() -> Result<Arc<RwLock<Self>>, StoreError> {
        // Create app dir (and parents)
        match get_app_config_dir().try_exists() {
            Ok(exists) => {
                if !exists {
                    std::fs::create_dir_all(get_app_config_dir()).map_err(StoreError::IO)?;
                }
            }
            Err(e) => return Err(StoreError::IO(e)),
        }

        // Initialize everything
        let database_file = get_app_config_dir().join(STORE_FILENAME);
        let db = Database::create(database_file.as_path()).map_err(StoreError::Database)?;

        let keyring = Keyring::new();
        let secret_key = Self::load_secret_key(&keyring);

        let store = Arc::new(RwLock::new(StoreManager {
            db,
            cache: HashMap::new(),
            secret_key,
            keyring,
        }));

        let cache = Self::build_cache(&store).await?;
        store.write().await.cache = cache;

        Ok(store)
    }

    /// Load secret key (for initialization).
    pub fn load_secret_key(keyring: &Keyring) -> SecretKey {
        if !keyring.key_exists(Keys::SecretKey, None) {
            info!("Generating new secret key...");
            let key = SecretKey::generate(&mut OsRng);

            keyring
                .set_key(Keys::SecretKey, key.to_string().as_str(), None)
                .unwrap();

            key
        } else {
            info!("Loading secret key from keyring");
            keyring.get_key(Keys::SecretKey, None).unwrap().parse().unwrap()
        }
    }

    /// Get shared folder keys.
    pub fn get_keys(keyring: &Keyring, uuid: Uuid) -> Option<(Option<WriteKey>, ReadKey)> {
        if let Ok(key) = keyring.get_key(Keys::SharedDirKey, Some(uuid.to_string().as_str())) {
            let mut split_key = key.split(" ");

            // Decode signing key
            let sign_key = {
                if let Some(raw) = split_key.next() {
                    if raw.contains("*") {
                        None
                    } else {
                        match BASE64_STANDARD.decode(raw) {
                            Ok(unencoded) => match WriteKey::try_from(unencoded.as_slice()) {
                                Ok(key) => Some(key),
                                Err(e) => {
                                    error!("Malformed SharedDirKey for {}: {} (sign key)", e, uuid);
                                    return None;
                                }
                            },
                            Err(e) => {
                                error!("Malformed SharedDirKey for {}: {} (sign key)", e, uuid);
                                return None;
                            }
                        }
                    }
                } else {
                    error!("Malformed SharedDirKey for {}", uuid);
                    return None;
                }
            };

            // Decode verifying key
            let verif_key = {
                if let Some(raw) = split_key.next() {
                    match BASE64_STANDARD.decode(raw) {
                        Ok(unencoded) => match ReadKey::try_from(unencoded.as_slice()) {
                            Ok(key) => key,
                            Err(e) => {
                                error!("Malformed SharedDirKey for {}: {} (verif key)", e, uuid);
                                return None;
                            }
                        },
                        Err(e) => {
                            error!("Malformed SharedDirKey for {}: {} (verif key)", e, uuid);
                            return None;
                        }
                    }
                } else {
                    error!("Malformed SharedDirKey for {}", uuid);
                    return None;
                }
            };

            Some((sign_key, verif_key))
        } else {
            error!("Cannot load SharedDirKey for {}", uuid);
            None
        }
    }

    /// Build [`SharedDirectory`] cache (for initialization).
    pub async fn build_cache(store: &Arc<RwLock<StoreManager>>) -> Result<HashMap<Uuid, SharedDirectory>, StoreError> {
        info!("Building store cache...");
        let mut cache = HashMap::new();

        let transaction = store.write().await.get_write_transaction()?;
        let keyring = &store.read().await.keyring;

        {
            let base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            let head_table = transaction.open_table(HEAD_TABLE).map_err(StoreError::Table)?;
            let local_tree_table = transaction.open_table(LOCAL_TREE_TABLE).map_err(StoreError::Table)?;
            let neighbors_table = transaction.open_table(NEIGHBORS_TABLE).map_err(StoreError::Table)?;
            let local_provision_table = transaction
                .open_multimap_table(LOCAL_PROVISIONS_TABLE)
                .map_err(StoreError::Table)?;
            let remote_provisions_table = transaction
                .open_multimap_table(REMOTE_PROVISIONS_TABLE)
                .map_err(StoreError::Table)?;

            for range in base_table.iter().map_err(StoreError::Storage)? {
                let (path, uuid_bytes) = range.unwrap();
                let head_opt = head_table.get(uuid_bytes.value()).map_err(StoreError::Storage)?;
                let local_tree_opt = local_tree_table.get(uuid_bytes.value()).map_err(StoreError::Storage)?;
                let neighbors_opt = neighbors_table.get(uuid_bytes.value()).map_err(StoreError::Storage)?;
                let local_provisions = local_provision_table
                    .get(uuid_bytes.value())
                    .map_err(StoreError::Storage)?;
                let remote_provisions = remote_provisions_table
                    .get(uuid_bytes.value())
                    .map_err(StoreError::Storage)?;

                let uuid = Uuid::from_bytes(uuid_bytes.value());

                if let (Some(head), Some(neighbors), Some(tree)) = (head_opt, neighbors_opt, local_tree_opt) {
                    if let Some((sign_key, verif_key)) = Self::get_keys(&keyring, uuid) {
                        let uuid_string = uuid.to_string();

                        // Reading state table
                        let state_table_def: TableDefinition<[u8; 32], Delta> =
                            TableDefinition::new(uuid_string.as_str());
                        if transaction
                            .list_tables()
                            .map_err(StoreError::Storage)?
                            .map(|e| e.name().to_string())
                            .any(|e| e == uuid.to_string())
                        {
                            let state_table = transaction.open_table(state_table_def).map_err(StoreError::Table)?;

                            let (local_provisions, remote_provisions) =
                                Self::load_provisions(local_provisions, remote_provisions)?;

                            // Load neighbors
                            let mut neighbors_map = HashMap::new();
                            for n in neighbors.value() {
                                if let Ok(node_id) = NodeId::from_bytes(&n) {
                                    neighbors_map.insert(node_id, false);
                                }
                            }

                            // Build state
                            info!("Building state for {}", uuid);
                            if let Some(state) = State::from_table(&state_table, head.value()) {
                                cache.insert(
                                    uuid,
                                    SharedDirectory {
                                        uuid,
                                        path: PathBuf::from(path.value()),
                                        state: StoreLock::new(&store, state, uuid),
                                        local_tree: StoreLock::new(&store, tree.value(), uuid),
                                        neighbors: StoreLock::new(&store, neighbors_map, uuid),
                                        local_provisions: StoreLock::new(&store, local_provisions, uuid),
                                        remote_provisions: StoreLock::new(&store, remote_provisions, uuid),
                                        handle: Arc::new(RwLock::new(None)),
                                        initial_sync: Arc::new(RwLock::new(false)),
                                        write_key: sign_key,
                                        read_key: verif_key,
                                    },
                                );
                            } else {
                                warn!("Failed!");
                            }
                        } else {
                            error!("Cannot load state for {}: Table not found", uuid);
                        }
                    }
                } else {
                    error!(
                        "Cannot load state for {}: Head, Neighbors or Local Tree are missing",
                        uuid
                    );
                }
            }
        }

        transaction.commit().map_err(StoreError::Commit)?;

        Ok(cache)
    }

    /// Load the [`DownloadJob`]s (for initialization).
    pub async fn load_jobs(
        store: &Arc<RwLock<StoreManager>>,
    ) -> Result<HashMap<Hash, StoreLock<DownloadJob>>, StoreError> {
        let mut jobs = HashMap::new();

        let transaction = store.write().await.get_write_transaction()?;

        {
            let jobs_table = transaction.open_table(JOBS_TABLE).map_err(StoreError::Table)?;

            for (_, job_access) in jobs_table.iter().map_err(StoreError::Storage)?.flatten() {
                let job = job_access.value();
                jobs.insert(*job.hash(), StoreLock::new(store, job, ()));
            }
        }

        Ok(jobs)
    }

    /// Load the local and remote [`Provision`]s of a [`SharedDirectory`] (for initialization).
    fn load_provisions(
        local_provision: MultimapValue<LocalProvision>,
        remote_provisions: MultimapValue<RemoteProvision>,
    ) -> Result<(LocalProvisionsMap, RemoteProvisionsMap), StoreError> {
        let mut local = HashMap::new();
        let mut remote = HashMap::new();

        for provision_access in local_provision.into_iter().flatten() {
            let provision = provision_access.value();
            local.insert(provision.hash(), provision);
        }

        for provision_access in remote_provisions.into_iter().flatten() {
            let provision = provision_access.value();
            if let Some(node_id) = provision.node_id() {
                remote
                    .entry(*provision.hash())
                    .or_insert(HashMap::new())
                    .insert(node_id, provision);
            }
        }

        Ok((local, remote))
    }

    /// Add a [`SharedDirectory`] to the store.
    pub async fn add_shared_dir(&mut self, dir: &SharedDirectory) -> Result<(), StoreError> {
        let sign_key_base64: String = {
            match dir.write_key.clone() {
                Some(key) => BASE64_STANDARD.encode(key.to_bytes()),
                None => String::from("*"),
            }
        };
        let verif_key_base64 = BASE64_STANDARD.encode(dir.read_key);

        self.keyring
            .set_key(
                Keys::SharedDirKey,
                (sign_key_base64 + " " + verif_key_base64.as_str()).as_str(),
                Some(dir.uuid.to_string().as_str()),
            )
            .map_err(StoreError::Keyring)?;

        self.cache.insert(dir.uuid, dir.clone());

        // Adding base tables
        let transaction = self.get_write_transaction()?;
        {
            let mut base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            base_table
                .insert(dir.path.to_string_lossy().as_ref(), dir.uuid.as_bytes())
                .map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)?;

        Ok(())
    }

    /// Remove a [`SharedDirectory`] from store.
    pub async fn remove_shared_dir(&mut self, dir: &SharedDirectory) -> Result<(), StoreError> {
        self.cache.remove(&dir.uuid);

        self.keyring
            .delete_key(Keys::SharedDirKey, Some(dir.uuid.to_string().as_str()))
            .map_err(StoreError::Keyring)?;

        // Removing tables
        let transaction = self.get_write_transaction()?;
        {
            let mut base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            let mut head_table = transaction.open_table(HEAD_TABLE).map_err(StoreError::Table)?;
            let mut local_tree_table = transaction.open_table(LOCAL_TREE_TABLE).map_err(StoreError::Table)?;
            let mut neighbor_table = transaction.open_table(NEIGHBORS_TABLE).map_err(StoreError::Table)?;
            let mut local_provision_table = transaction
                .open_multimap_table(LOCAL_PROVISIONS_TABLE)
                .map_err(StoreError::Table)?;
            let mut remote_provisions_table = transaction
                .open_multimap_table(REMOTE_PROVISIONS_TABLE)
                .map_err(StoreError::Table)?;

            base_table
                .remove(dir.path.to_string_lossy().as_ref())
                .map_err(StoreError::Storage)?;
            head_table.remove(dir.uuid.as_bytes()).map_err(StoreError::Storage)?;
            local_tree_table
                .remove(dir.uuid.as_bytes())
                .map_err(StoreError::Storage)?;
            neighbor_table
                .remove(dir.uuid.as_bytes())
                .map_err(StoreError::Storage)?;
            local_provision_table
                .remove_all(dir.uuid.as_bytes())
                .map_err(StoreError::Storage)?;
            remote_provisions_table
                .remove_all(dir.uuid.as_bytes())
                .map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)?;

        Ok(())
    }

    /// Get a specific [`SharedDirectory`].
    pub fn get_shared_dir(&self, uuid: &Uuid) -> Option<SharedDirectory> {
        self.cache.get(uuid).cloned()
    }

    /// Get all [`SharedDirectory`].
    pub fn get_all_dirs(&self) -> Vec<SharedDirectory> {
        self.cache.values().cloned().collect()
    }

    /// Get a write transaction for the database.
    pub fn get_write_transaction(&self) -> Result<WriteTransaction, StoreError> {
        self.db.begin_write().map_err(StoreError::Transaction)
    }

    pub fn secret_key(&self) -> SecretKey {
        self.secret_key.clone()
    }

    pub fn public_key(&self) -> PublicKey {
        self.secret_key.public()
    }
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("{0}")]
    IO(std::io::Error),

    #[error("{0}")]
    Database(DatabaseError),

    #[error("{0}")]
    Transaction(TransactionError),

    #[error("{0}")]
    Table(TableError),

    #[error("{0}")]
    Storage(StorageError),

    #[error("{0}")]
    Commit(CommitError),

    #[error("{0}")]
    Serialize(rkyv::rancor::Error),

    #[error("{0}")]
    Keyring(::keyring::error::Error),
}
