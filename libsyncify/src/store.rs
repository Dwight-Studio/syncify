use crate::engine::state::State;
use crate::store::keyring::{Keyring, Keys};
use crate::{get_app_dir, SharedDirectory};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use chacha20poly1305::aead::OsRng;
use ed25519_dalek::{SigningKey, VerifyingKey};
use iroh::SecretKey;
use ::keyring::Error;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub mod keyring;

const MAX_RENAME_ATTEMPTS: u16 = 256;
const STORE_FILENAME: &str = "store.toml";

/// Entity holding the non-sensitive shared folder data.
#[derive(Serialize, Deserialize, Clone)]
pub struct DataStore {
    pub store_version: String,
    pub shared_directories: HashMap<String, SharedDirectoryData>,
}

/// Store manager.
pub struct StoreManager {
    cache: HashMap<Uuid, SharedDirectory>,
    secret_key: SecretKey,
    keyring: Keyring,
    file_path: PathBuf,
}

impl StoreManager {
    pub async fn new() -> Result<Self, io::Error> {
        // Create app dir (and parents)
        if !get_app_dir().exists() {
            tokio::fs::create_dir_all(&get_app_dir()).await?;
        }

        // Initialize everything
        let store_file = get_app_dir().join(STORE_FILENAME);
        let keyring = Keyring::new();
        let secret_key = Self::load_secret_key(&keyring);
        let cache = Self::build_cache(&keyring, Self::load_data_store(&store_file)?);

        Ok(StoreManager {
            cache,
            secret_key,
            keyring,
            file_path: store_file,
        })
    }

    /// Load data store (for initialization).
    fn load_data_store(store_file: &PathBuf) -> Result<DataStore, io::Error> {
        // Check if the store file exists
        if Path::exists(store_file.as_path()) {
            // If so, read it
            info!("Reading store file...");
            let file_content: &mut String = &mut "".to_string();
            File::open(store_file.as_path())?.read_to_string(file_content)?;

            let result: Result<DataStore, toml::de::Error> = toml::from_str(file_content);

            // Check if the store is parsable
            match result {
                Ok(res) => return Ok(res),
                Err(_) => {
                    warn!("Invalid store file: {}", store_file.display());
                    let mut i = 0;
                    while std::fs::exists(
                        get_app_dir()
                            .join(format!("{STORE_FILENAME}.backup{i}"))
                            .as_path(),
                    )? {
                        i += 1;
                        if i >= MAX_RENAME_ATTEMPTS {
                            break;
                        }
                    }
                    if i < MAX_RENAME_ATTEMPTS {
                        std::fs::rename(
                            store_file.as_path(),
                            get_app_dir()
                                .join(format!("{STORE_FILENAME}.backup{i}"))
                                .as_path(),
                        )?;
                    } else {
                        warn!("Unable backup store!");
                        return Err(io::ErrorKind::AlreadyExists.into());
                    }
                }
            }
        };

        Ok(DataStore {
            store_version: env!("CARGO_PKG_VERSION").to_string(),
            shared_directories: HashMap::new(),
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
    fn build_cache(keyring: &Keyring, store_data: DataStore) -> HashMap<Uuid, SharedDirectory> {
        let mut cache = HashMap::new();

        for (uuid_str, dir_data) in store_data.shared_directories {
            if let Ok(uuid) = Uuid::try_from(uuid_str.as_str()) {
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
                        path: PathBuf::from(&dir_data.path),
                        stored_data: Arc::new(RwLock::new(dir_data)),
                        sign_key,
                        verif_key,
                    });
                } else {
                    error!("Unable to load SharedDirKey for {}", uuid)
                }
            } else {
                error!("Unable to read UUID '{}'", uuid_str);
            }
        }
        cache
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
        self.save().await;
        Ok(())
    }

    /// Remove a [`SharedDirectory`] from store.
    pub async fn remove_shared_dir(&mut self, dir: &SharedDirectory) -> Result<(), Error> {
        self.cache.remove(&dir.uuid);

        self.keyring
            .delete_key(Keys::SharedDirKey, Some(dir.uuid.to_string().as_str()))?;

        self.save().await;
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

    /// Save cache to file.
    pub async fn save(&self) {
        info!("Saving store...");

        // Translate HashMap
        let mut serial_map = HashMap::new();
        for (uuid, dir) in &self.cache {
            serial_map.insert(uuid.to_string(), dir.stored_data.read().await.clone());
        }

        let store_data = DataStore {
            store_version: env!("CARGO_PKG_VERSION").to_string(),
            shared_directories: serial_map,
        };

        let toml_data = toml::to_string(&store_data).unwrap();
        let mut file = File::create(self.file_path.as_path()).unwrap();
        file.write_all(toml_data.as_bytes()).unwrap();
    }

    pub fn secret_key(&self) -> SecretKey {
        self.secret_key.clone()
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SharedDirectoryData {
    pub(crate) path: String,
    pub(crate) state: State,
}

impl Display for SharedDirectoryData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.clone())
    }
}

impl Clone for SharedDirectoryData {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            state: self.state.clone(),
        }
    }
}