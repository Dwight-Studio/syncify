use crate::{get_app_dir, SharedDirectory};
use crate::store::keyring::{Keyring, Keys};
use chacha20poly1305::aead::{Key, OsRng};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use iroh::SecretKey;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::Path;
use uuid::{Bytes, Uuid};

pub mod keyring;

const MAX_RENAME_ATTEMPTS: u16 = 256;
const CONFIG_FILENAME: &str = "store.toml";

/// Entity holding the non-sensitive shared folder data.
#[derive(Serialize, Deserialize)]
pub struct Store {
    pub shared_directories: Vec<SharedDirectoryData>,
}

/// Store manager.
pub struct StoreManager {
    pub secret_key: SecretKey,
    pub data: Store,
    keyring: Keyring,
}

impl Display for StoreManager {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "(secret_key: {})", self.secret_key)
    }
}

impl StoreManager {
    pub async fn new() -> Result<Self, io::Error> {
        // Create app dir (and parents)
        if !get_app_dir().exists() {
            tokio::fs::create_dir_all(&get_app_dir()).await?;
        }

        let config_file = get_app_dir().join(CONFIG_FILENAME);

        // Create store file if it does not exist
        // Load the store file if it exists
        let syncify_config_data: Store = {
            if Path::exists(config_file.as_path()) {
                info!("Reading store file...");
                let file_content: &mut String = &mut "".to_string();
                File::open(config_file.as_path())?.read_to_string(file_content)?;

                let result = toml::from_str(file_content);

                // Check if the store is parsable
                if let Ok(data) = result {
                    data
                } else {
                    warn!("Invalid store file: {}", config_file.display());
                    let mut i = 0;
                    while std::fs::exists(
                        get_app_dir()
                            .join(format!("{CONFIG_FILENAME}.backup{i}"))
                            .as_path(),
                    )? {
                        i += 1;
                        if i >= MAX_RENAME_ATTEMPTS {
                            break;
                        }
                    }
                    if i < MAX_RENAME_ATTEMPTS {
                        std::fs::rename(
                            config_file.as_path(),
                            config_file.join(format!(".backup{}", &i)).as_path(),
                        )?;
                    } else {
                        warn!("Unable backup store!");
                        return Err(io::ErrorKind::AlreadyExists.into());
                    }
                }
            }

            info!("Config file does not exists, creating a new one...");
            let mut w_file = File::create(config_file.as_path())?;
            let syncify_config_data = Store {
                shared_directories: vec![],
            };

            w_file.write_all(toml::to_string(&syncify_config_data).unwrap().as_bytes())?;

            syncify_config_data
        };

        let syncify_keyring = Keyring::new();
        let secret_key = {
            if !syncify_keyring.key_exists(Keys::SecretKey, None) {
                info!("Generating new secret key...");
                let mut rng = rand::rngs::OsRng;
                let key = SecretKey::generate(&mut rng);

                syncify_keyring
                    .set_key(Keys::SecretKey, key.to_string().as_str(), None)
                    .unwrap();

                key
            } else {
                info!("Key already exists");
                syncify_keyring
                    .get_key(Keys::SecretKey, None)
                    .unwrap()
                    .parse()
                    .unwrap()
            }
        };

        Ok(StoreManager {
            secret_key,
            data: syncify_config_data,
            keyring: syncify_keyring,
        })
    }

    pub fn add_shared_dir(&mut self, dir: &SharedDirectory) {
        self.data.shared_directories.push(dir.data.clone());
        self.keyring
            .set_key(
                Keys::SharedDirKey,
                &String::from_utf8_lossy(XChaCha20Poly1305::generate_key(&mut OsRng).as_slice()),
                Some(dir.data.uuid.to_string().as_str()),
            )
            .unwrap();
    }

    pub fn remove_shared_dir(&mut self, dir: &SharedDirectory) {
        //let dzqqdzqd: Key<XChaCha20Poly1305> = ;
        self.data
            .shared_directories
            .retain(|shared_directory| !dir.data.uuid.eq(&shared_directory.uuid));
        self.keyring
            .delete_key(Keys::SharedDirKey, Some(dir.data.uuid.to_string().as_str()))
            .unwrap();
    }
    
    pub fn get_shared_dir(&self, uuid: &Uuid) -> Option<SharedDirectory> {
        let dir_data = self.data.shared_directories
            .iter()
            .find(|folder| folder.uuid == *uuid)
            .cloned();
        
        if let Ok(key) = self.keyring.get_key(Keys::SharedDirKey, Some(uuid.to_string().as_str())) {
            dir_data.map(|data| SharedDirectory { data, key: *Key::<XChaCha20Poly1305>::from_slice(key.as_bytes()) })
        } else { None }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Uuid")]
pub struct UuidDef(#[serde(getter = "Uuid::as_bytes")] Bytes);

impl From<UuidDef> for Uuid {
    fn from(uuid: UuidDef) -> Self {
        Uuid::from_bytes(uuid.0)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SharedDirectoryData {
    #[serde(with = "UuidDef")]
    pub(crate) uuid: Uuid,
    pub(crate) path: String,
}

impl Display for SharedDirectoryData {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.clone())
    }
}

impl Clone for SharedDirectoryData {
    fn clone(&self) -> Self {
        Self {
            uuid: self.uuid,
            path: self.path.clone(),
        }
    }
}

impl PartialEq<SharedDirectoryData> for SharedDirectoryData {
    fn eq(&self, other: &SharedDirectoryData) -> bool {
        self.uuid.eq(&other.uuid)
    }
}
