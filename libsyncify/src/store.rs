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
use crate::{LocalProvisionsMap, RemoteProvisionsMap, SharedDirectory, get_app_config_dir};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use blake3::Hash;
use chacha20poly1305::aead::OsRng;
use chrono::{DateTime, TimeDelta, Utc};
use ed25519_dalek::{SigningKey, VerifyingKey};
use iroh::SecretKey;
use iroh_base::{NodeId, PublicKey};
use log::{error, info, warn};
use redb::{
    CommitError, Database, DatabaseError, MultimapTableDefinition, MultimapValue, ReadableMultimapTable, ReadableTable,
    StorageError, Table, TableDefinition, TableError, TableHandle, TransactionError, WriteTransaction,
};
use std::collections::HashMap;
use std::ops::Deref;
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
/// Time after which a job is no longer automatically loaded into memory.
pub const JOBS_EXPIRATION: TimeDelta = TimeDelta::days(7);

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
    /// The timestamp is dated from last time the store was flushed.
    timestamp: DateTime<Utc>,
    db: Database,
    cache: HashMap<Uuid, SharedDirectory>,
    jobs: HashMap<Hash, Arc<RwLock<DownloadJob>>>,
    active_jobs: Vec<Arc<RwLock<DownloadJob>>>,
    secret_key: SecretKey,
    keyring: Keyring,
}

impl StoreManager {
    pub async fn new() -> Result<Arc<RwLock<Self>>, StoreError> {
        // Create app dir (and parents)
        if !get_app_config_dir().exists() {
            std::fs::create_dir_all(get_app_config_dir()).map_err(StoreError::IO)?;
        }

        // Initialize everything
        let database_file = get_app_config_dir().join(STORE_FILENAME);
        let db = Database::create(database_file.as_path()).map_err(StoreError::Database)?;

        let keyring = Keyring::new();
        let secret_key = Self::load_secret_key(&keyring);
        let (jobs, active_jobs) = Self::load_jobs(&db, Utc::now() - JOBS_EXPIRATION)?;

        let store = Arc::new(RwLock::new(StoreManager {
            timestamp: Utc::now(),
            db,
            cache: HashMap::new(),
            jobs,
            active_jobs,
            secret_key,
            keyring,
        }));

        let cache = Self::build_cache(&store, &store.read().await.keyring, &store.read().await.db)?;
        store.write().await.cache = cache;

        Ok(store)
    }

    /// Load secret key (for initialization).
    fn load_secret_key(keyring: &Keyring) -> SecretKey {
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
    fn get_keys(keyring: &Keyring, uuid: Uuid) -> Option<(Option<SigningKey>, VerifyingKey)> {
        if let Ok(key) = keyring.get_key(Keys::SharedDirKey, Some(uuid.to_string().as_str())) {
            let mut split_key = key.split(" ");

            // Decode signing key
            let sign_key = {
                if let Some(raw) = split_key.next() {
                    if raw.contains("*") {
                        None
                    } else {
                        match BASE64_STANDARD.decode(raw) {
                            Ok(unencoded) => match SigningKey::try_from(unencoded.as_slice()) {
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
                        Ok(unencoded) => match VerifyingKey::try_from(unencoded.as_slice()) {
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
            error!("Unable to load SharedDirKey for {}", uuid);
            None
        }
    }

    /// Build [`SharedDirectory`] cache (for initialization).
    fn build_cache(
        store: &Arc<RwLock<StoreManager>>,
        keyring: &Keyring,
        db: &Database,
    ) -> Result<HashMap<Uuid, SharedDirectory>, StoreError> {
        info!("Building store cache...");
        let mut cache = HashMap::new();

        let transaction = db.begin_write().map_err(StoreError::Transaction)?;

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
                            error!("Unable to load state for {}: Table not found", uuid);
                        }
                    }
                } else {
                    error!(
                        "Unable to load state for {}: Head, Neighbors or Local Tree are missing",
                        uuid
                    );
                }
            }
        }

        transaction.commit().map_err(StoreError::Commit)?;

        Ok(cache)
    }

    /// Load the [`DownloadJob`]s (for initialization).
    fn load_jobs(
        db: &Database,
        since: DateTime<Utc>,
    ) -> Result<(HashMap<Hash, Arc<RwLock<DownloadJob>>>, Vec<Arc<RwLock<DownloadJob>>>), StoreError> {
        let mut jobs = HashMap::new();
        let mut active_jobs = Vec::new();

        let transaction = db.begin_write().map_err(StoreError::Transaction)?;

        {
            let jobs_table = transaction.open_table(JOBS_TABLE).map_err(StoreError::Table)?;

            for (hash_access, job_access) in (jobs_table.iter().map_err(StoreError::Storage)?).flatten() {
                let hash = Hash::from_bytes(hash_access.value());
                let job = job_access.value();

                // If it is still active, add in the active vec
                if job.is_active() {
                    let job_ref = Arc::new(RwLock::new(job));
                    active_jobs.push(job_ref.clone());
                    jobs.insert(hash, job_ref);
                } else if *job.issued() > since {
                    jobs.insert(hash, Arc::new(RwLock::new(job)));
                }
            }
        }

        Ok((jobs, active_jobs))
    }

    /// Save the [`DownloadJob`]s.
    async fn flush_jobs(
        jobs: &mut HashMap<Hash, Arc<RwLock<DownloadJob>>>,
        jobs_table: &mut Table<'_, [u8; 32], DownloadJob>,
    ) {
        for (hash, job_ref) in jobs.iter() {
            if jobs_table
                .insert(hash.as_bytes(), job_ref.read().await.deref())
                .is_err()
            {
                error!("Unable to flush job {}", hash);
            }
        }

        let since: DateTime<Utc> = Utc::now() - JOBS_EXPIRATION;

        // Drop all old jobs
        jobs.retain(|_, j| {
            if let Ok(job) = j.try_read() {
                job.issued() > &since
            } else {
                true
            }
        })
    }

    pub async fn add_download_job(&mut self, download_job: Arc<RwLock<DownloadJob>>) {
        self.jobs
            .insert(*download_job.read().await.hash(), download_job.clone());
        self.active_jobs.push(download_job);
    }

    pub async fn get_download_job(&self, file_hash: Hash) -> Option<Arc<RwLock<DownloadJob>>> {
        for job in self.active_jobs.clone() {
            if *job.read().await.hash() == file_hash {
                return Some(job);
            }
        }

        None
    }

    pub fn get_download_jobs(&self) -> Vec<Arc<RwLock<DownloadJob>>> {
        self.active_jobs.clone()
    }
    
    pub async fn flush_download_jobs(&mut self) {
        let mut new_active_jobs: Vec<Arc<RwLock<DownloadJob>>> = Vec::new();
        
        for job in self.active_jobs.clone() {
            if job.read().await.is_active() {
                new_active_jobs.push(job);
            }
        }
        
        self.active_jobs = new_active_jobs;
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
                    .entry(provision.hash())
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

        // Flushing store
        self.flush().await?;

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

        // Flushing store
        self.flush().await?;

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

    //noinspection RsTraitObligations
    /// Flush cache to database.
    pub async fn flush(&mut self) -> Result<(), StoreError> {
        info!("Saving store...");

        let transaction = self.db.begin_write().map_err(StoreError::Transaction)?;

        {
            let mut jobs_table = transaction.open_table(JOBS_TABLE).map_err(StoreError::Table)?;

            Self::flush_jobs(&mut self.jobs, &mut jobs_table).await;
        }

        transaction.commit().map_err(StoreError::Commit)?;

        self.timestamp = Utc::now();

        info!("Save complete");

        Ok(())
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
