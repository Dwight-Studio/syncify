use crate::store::keyring::{Keyring, Keys};
use crate::{get_app_dir, SharedDirectory};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use chacha20poly1305::aead::OsRng;
use ed25519_dalek::{SigningKey, VerifyingKey};
use iroh::SecretKey;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::{Bytes, Uuid};

pub mod keyring;

const MAX_RENAME_ATTEMPTS: u16 = 256;
const STORE_FILENAME: &str = "store.toml";

/// Entity holding the non-sensitive shared folder data.
#[derive(Serialize, Deserialize)]
#[derive(Clone)]
pub struct Store {
    pub store_version: String,
    pub shared_directories: Vec<SharedDirectoryData>,
}

/// Store manager.
pub struct StoreManager {
    pub secret_key: SecretKey,
    pub data: Store,
    keyring: Keyring,
    file_path: PathBuf,
}

impl StoreManager {
    pub async fn new() -> Result<Self, io::Error> {
        // Create app dir (and parents)
        if !get_app_dir().exists() {
            tokio::fs::create_dir_all(&get_app_dir()).await?;
        }

        let store_file = get_app_dir().join(STORE_FILENAME);


        let mut store_data: Option<Store> = None;

        // Check if the store file exists
        if Path::exists(store_file.as_path()) {
            // If so, read it
            info!("Reading store file...");
            let file_content: &mut String = &mut "".to_string();
            File::open(store_file.as_path())?.read_to_string(file_content)?;

            let result: Result<Store, toml::de::Error> = toml::from_str(file_content);

            // Check if the store is parsable
            match result {
                Ok(res) => store_data = Some(res),
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
        }

        if store_data.is_none() {
            info!("Store file does not exists, creating a new one...");

            store_data = Some(Store {
                store_version: env!("CARGO_PKG_VERSION").to_string(),
                shared_directories: vec![],
            });
            let toml_data = toml::to_string(&store_data.clone().unwrap()).unwrap();
            let mut file = File::create(store_file.as_path())?;
            file.write_all(toml_data.as_bytes())?;
        }

        let keyring = Keyring::new();
        let secret_key = {
            if !keyring.key_exists(Keys::SecretKey, None) {
                info!("Generating new secret key...");
                let key = SecretKey::generate(&mut OsRng);

                keyring
                    .set_key(Keys::SecretKey, key.to_string().as_str(), None)
                    .unwrap();

                key
            } else {
                info!("Key already exists");
                keyring
                    .get_key(Keys::SecretKey, None)
                    .unwrap()
                    .parse()
                    .unwrap()
            }
        };

        Ok(StoreManager {
            secret_key,
            data: store_data.unwrap(),
            keyring,
            file_path: store_file,
        })
    }

    pub fn add_shared_dir(&mut self, dir: &SharedDirectory) {
        let sign_key_base64: String = {
            match dir.sign_key.clone() {
                Some(key) => BASE64_STANDARD.encode(key.to_bytes()),
                None => String::from("*"),
            }
        };
        let verif_key_base64 = BASE64_STANDARD.encode(dir.verif_key);

        self.data.shared_directories.push(dir.data.clone());
        self.keyring
            .set_key(
                Keys::SharedDirKey,
                (sign_key_base64 + " " + verif_key_base64.as_str()).as_str(),
                Some(dir.data.uuid.to_string().as_str()),
            )
            .unwrap();
        self.save();
    }

    pub fn remove_shared_dir(&mut self, dir: &SharedDirectory) {
        self.data
            .shared_directories
            .retain(|shared_directory| !dir.data.uuid.eq(&shared_directory.uuid));
        self.keyring
            .delete_key(Keys::SharedDirKey, Some(dir.data.uuid.to_string().as_str()))
            .unwrap();
    }

    pub fn get_shared_dir(&self, uuid: &Uuid) -> Option<SharedDirectory> {
        let dir_data = self
            .data
            .shared_directories
            .iter()
            .find(|folder| folder.uuid == *uuid)
            .cloned();

        if let Ok(key) = self
            .keyring
            .get_key(Keys::SharedDirKey, Some(uuid.to_string().as_str()))
        {
            let mut split_key = key.split(" ");
            let sign_key = {
                let raw = split_key.next();
                if raw.unwrap().contains("*") {
                    None
                } else {
                    Some(
                        SigningKey::try_from(
                            BASE64_STANDARD.decode(raw.unwrap()).unwrap().as_slice(),
                        )
                        .unwrap(),
                    )
                }
            };
            let verif_key = VerifyingKey::try_from(
                BASE64_STANDARD
                    .decode(split_key.next().unwrap())
                    .unwrap()
                    .as_slice(),
            )
            .unwrap();

            dir_data.map(|data| SharedDirectory {
                data,
                sign_key,
                verif_key,
            })
        } else {
            None
        }
    }

    pub fn get_all_dirs(&self) -> Vec<SharedDirectory> {
        self.data
            .shared_directories
            .iter()
            .map(|dir| self.get_shared_dir(&dir.uuid).unwrap())
            .collect()
    }

    pub fn get_all_dirs_data(&self) -> Vec<SharedDirectoryData> {
        self.data.shared_directories.clone()
    }

    fn save(&self) {
        let toml_data = toml::to_string(&self.data).unwrap();
        let mut file = File::create(self.file_path.as_path()).unwrap();
        file.write_all(toml_data.as_bytes()).unwrap();
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
        write!(f, "{} / {}", self.uuid, self.path.clone())
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
