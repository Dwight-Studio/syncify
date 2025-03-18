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

use crate::engine::serial_state::SerialState;
use crate::store::keyring::{Keyring, Keys};
use crate::{get_app_dir, InnerSharedDirectory, SharedDirectory};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use blake3::Hash;
use chacha20poly1305::aead::OsRng;
use ed25519_dalek::{SigningKey, VerifyingKey};
use iroh::SecretKey;
use ::keyring::Error;
use log::{error, info, warn};
use redb::{CommitError, Database, DatabaseError, ReadableTable, StorageError, TableDefinition, TableError, TableHandle, TransactionError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod keyring;
pub mod link;

const STORE_FILENAME: &str = "store.db";

// Base table linking Path to UUID
const BASE_TABLE: TableDefinition<&str, [u8; 16]> = TableDefinition::new("base");
const HEAD_TABLE: TableDefinition<[u8; 16], [u8; 32]> = TableDefinition::new("head");
const NEIGHBORS_TABLE: TableDefinition<[u8; 16], Vec<[u8; 32]>> = TableDefinition::new("neighbors");

/// Store manager.
pub struct StoreManager {
    cache: HashMap<Uuid, SharedDirectory>,
    secret_key: SecretKey,
    keyring: Keyring,
    file_path: PathBuf,
}

impl StoreManager {
    pub async fn new() -> Result<Self, StoreError> {
        // Create app dir (and parents)
        if !get_app_dir().exists() {
            tokio::fs::create_dir_all(&get_app_dir()).await.map_err(StoreError::IO)?;
        }

        // Initialize everything
        let database_file = get_app_dir().join(STORE_FILENAME);
        let keyring = Keyring::new();
        let secret_key = Self::load_secret_key(&keyring);
        let cache = Self::build_cache(&keyring, database_file.as_path())?;

        Ok(StoreManager {
            cache,
            secret_key,
            keyring,
            file_path: database_file,
        })
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
            keyring
                .get_key(Keys::SecretKey, None)
                .unwrap()
                .parse()
                .unwrap()
        }
    }

    /// Get shared folder keys.
    fn get_keys(keyring: &Keyring, uuid: Uuid) -> Option<(Option<SigningKey>, VerifyingKey)> {
        if let Ok(key) = keyring
            .get_key(Keys::SharedDirKey, Some(uuid.to_string().as_str()))
        {
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
                    return None
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
                    return None
                }
            };

            Some((sign_key, verif_key))
        } else {
            error!("Unable to load SharedDirKey for {}", uuid);
            None
        }
    }

    /// Build [`SharedDirectory`] cache (for initialization).
    fn build_cache(keyring: &Keyring, path: &Path) -> Result<HashMap<Uuid, SharedDirectory>, StoreError> {
        info!("Building store cache...");
        let mut cache = HashMap::new();

        let db = Database::create(path).map_err(StoreError::Database)?;
        let transaction = db.begin_write().map_err(StoreError::Transaction)?;

        {
            let base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            let head_table = transaction.open_table(HEAD_TABLE).map_err(StoreError::Table)?;
            let neighbors_table = transaction.open_table(NEIGHBORS_TABLE).map_err(StoreError::Table)?;

            for range in base_table.iter().map_err(StoreError::Storage)? {
                let (path, uuid_bytes) = range.unwrap();
                let opt_head = head_table.get(uuid_bytes.value()).map_err(StoreError::Storage)?;
                let opt_neighbors = neighbors_table.get(uuid_bytes.value()).map_err(StoreError::Storage)?;

                let uuid = Uuid::from_bytes(uuid_bytes.value());

                if let (Some(head), Some(neighbors)) = (opt_head, opt_neighbors) {
                    if let Some((sign_key, verif_key)) = Self::get_keys(&keyring, uuid) {

                        let uuid_string = uuid.to_string();

                        // Reading state table
                        let state_table_def: TableDefinition<[u8; 32], &[u8]> = TableDefinition::new(uuid_string.as_str());
                        if transaction.list_tables().map_err(StoreError::Storage)?
                            .map(|e| e.name().to_string()).any(|e| e == uuid.to_string()) {
                            let state_table = transaction.open_table(state_table_def).map_err(StoreError::Table)?;

                            // Build state
                            info!("Building state for {}", uuid);
                            if let Some(state) = SerialState::build_from_table(state_table, head.value()) {
                                cache.insert(uuid, SharedDirectory {
                                    uuid,
                                    path: PathBuf::from(path.value()),
                                    inner: Arc::new(RwLock::new(InnerSharedDirectory::new(
                                        state,
                                        neighbors.value().iter().map(|e| (*e, false)).collect()
                                    ))),
                                    sign_key,
                                    verif_key,
                                });
                            } else {
                                warn!("Failed!");
                            }
                        } else {
                            error!("Unable to load state for {}: Table not found", uuid);
                        }
                    }
                } else {
                    error!("Unable to load state for {}: Head not found", uuid);
                }
            }
        }

        transaction.commit().map_err(StoreError::Commit)?;
        
        Ok(cache)
    }

    /// Add a [`SharedDirectory`] to the store.
    pub async fn add_shared_dir(&mut self, dir: &SharedDirectory) -> Result<(), Error> {
        let sign_key_base64: String = {
            match dir.sign_key.clone() {
                Some(key) => BASE64_STANDARD.encode(key.to_bytes()),
                None => String::from("*"),
            }
        };
        let verif_key_base64 = BASE64_STANDARD.encode(dir.verif_key);

        self.keyring
            .set_key(
                Keys::SharedDirKey,
                (sign_key_base64 + " " + verif_key_base64.as_str()).as_str(),
                Some(dir.uuid.to_string().as_str()),
            )?;
        
        self.cache.insert(dir.uuid, dir.clone());
        self.flush().await;
        Ok(())
    }

    /// Remove a [`SharedDirectory`] from store.
    pub async fn remove_shared_dir(&mut self, dir: &SharedDirectory) -> Result<(), Error> {
        self.cache.remove(&dir.uuid);

        self.keyring
            .delete_key(Keys::SharedDirKey, Some(dir.uuid.to_string().as_str()))?;

        self.flush().await;
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

    //noinspection RsTraitObligations
    /// Flush cache to database.
    pub async fn flush(&self) -> Result<(), StoreError> {
        info!("Saving store...");

        let db = Database::create(self.file_path.as_path()).map_err(StoreError::Database)?;
        let transaction = db.begin_write().map_err(StoreError::Transaction)?;

        {
            let mut base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            let mut head_table = transaction.open_table(HEAD_TABLE).map_err(StoreError::Table)?;
            let mut neighbor_table = transaction.open_table(NEIGHBORS_TABLE).map_err(StoreError::Table)?;

            // Save each SharedDirectory
            for (uuid, dir) in &self.cache {
                info!("Saving state for {}", uuid);
                let mut inner = dir.inner.write().await;

                // Update index tables
                base_table.insert(dir.path.to_string_lossy().as_ref(), uuid.as_bytes()).map_err(StoreError::Storage)?;
                head_table.insert(uuid.as_bytes(), inner.state.hash().as_bytes()).map_err(StoreError::Storage)?;
                neighbor_table.insert(uuid.as_bytes(), inner.neighbors.keys().map(|e| *e).collect::<Vec<[u8; 32]>>()).map_err(StoreError::Storage)?;

                let uuid_string = uuid.to_string();

                let state_table_def: TableDefinition<[u8; 32], &[u8]> = TableDefinition::new(uuid_string.as_str());
                let mut state_table = transaction.open_table(state_table_def).map_err(StoreError::Table)?;
                
                let serial_state = SerialState::from(&inner.state);

                for (hash, serial_delta) in serial_state.pool() {
                    match rkyv::to_bytes::<rkyv::rancor::Error>(&serial_delta) {
                        Ok(value) => {
                            state_table.insert(hash, value.as_slice()).map_err(StoreError::Storage)?
                        },
                        Err(e) => {
                            error!("Could not serialize {} in {}", Hash::from_bytes(hash), uuid);
                            return Err(StoreError::Serialize(e))
                        },
                    };
                }

                if inner.state.prune() {
                    info!("Pruned state {}", uuid_string);
                }
            }
        }

        transaction.commit().map_err(StoreError::Commit)?;

        Ok(())
    }

    pub fn secret_key(&self) -> SecretKey {
        self.secret_key.clone()
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
    Serialize(rkyv::rancor::Error)
}