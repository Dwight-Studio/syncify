use crate::engine::state::{SerialState, State};
use crate::store::keyring::{Keyring, Keys};
use crate::{get_app_dir, SharedDirectory};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use chacha20poly1305::aead::OsRng;
use iroh::SecretKey;
use ::keyring::Error;
use log::{error, info};
use redb::{CommitError, Database, DatabaseError, ReadableTable, StorageError, TableDefinition, TableError, TransactionError};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ed25519_dalek::{SigningKey, VerifyingKey};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod keyring;
pub mod link;

const MAX_RENAME_ATTEMPTS: u16 = 256;
const STORE_FILENAME: &str = "store.db";

// Base table linking Path to UUID
const BASE_TABLE: TableDefinition<&str, [u8; 16]> = TableDefinition::new("base");
const STATE_TABLE: TableDefinition<[u8; 16], SerialState> = TableDefinition::new("state");

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

    /// Build [`SharedDirectory`] cache (for initialization).
    fn build_cache(keyring: &Keyring, path: &Path) -> Result<HashMap<Uuid, SharedDirectory>, StoreError> {
        info!("Building store cache...");
        let mut cache = HashMap::new();

        let db = Database::create(path).map_err(StoreError::Database)?;
        let transaction = db.begin_write().map_err(StoreError::Transaction)?;

        {
            let base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            let state_table = transaction.open_table(STATE_TABLE).map_err(StoreError::Table)?;

            for range in base_table.iter().map_err(StoreError::Storage)? {
                let (path, uuid_bytes) = range.unwrap();
                let serial_state = state_table.get(uuid_bytes.value()).map_err(StoreError::Storage)?;

                if serial_state.is_none() {
                    error!("Unable to load state (not in state table)")
                } else {
                    let uuid = Uuid::from_bytes(uuid_bytes.value());
                    let state = State::from(serial_state.unwrap().value());

                    info!("Loading state for {}", uuid);

                    
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
                                                continue;
                                            }
                                        },
                                        Err(e) => {
                                            error!("Malformed SharedDirKey for {}: {} (sign key)", e, uuid);
                                            continue;
                                        }
                                    }
                                }
                            } else {
                                error!("Malformed SharedDirKey for {}", uuid);
                                continue
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
                                            continue;
                                        }
                                    },
                                    Err(e) => {
                                        error!("Malformed SharedDirKey for {}: {} (verif key)", e, uuid);
                                        continue;
                                    }
                                }
                            } else {
                                error!("Malformed SharedDirKey for {}", uuid);
                                continue
                            }
                        };

                        cache.insert(uuid, SharedDirectory {
                            uuid,
                            path: PathBuf::from(path.value()),
                            state: Arc::new(RwLock::new(state)),
                            sign_key,
                            verif_key,
                        });
                    } else {
                        error!("Unable to load SharedDirKey for {}", uuid);
                    }
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

    /// Flush cache to database.
    pub async fn flush(&self) -> Result<(), StoreError> {
        info!("Saving store...");

        let db = Database::create(self.file_path.as_path()).map_err(StoreError::Database)?;
        let transaction = db.begin_write().map_err(StoreError::Transaction)?;

        {
            let mut base_table = transaction.open_table(BASE_TABLE).map_err(StoreError::Table)?;
            let mut state_table = transaction.open_table(STATE_TABLE).map_err(StoreError::Table)?;

            for (uuid, dir) in &self.cache {
                base_table.insert(dir.path.to_string_lossy().as_ref(), uuid.as_bytes()).map_err(StoreError::Storage)?;

                state_table.insert(uuid.as_bytes(), SerialState::from(dir.state.read().await.deref())).map_err(StoreError::Storage)?;
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
    Commit(CommitError)
}