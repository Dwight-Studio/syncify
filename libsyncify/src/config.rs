use crate::config::keyring::{Keys, SyncifyKeyring};
use crate::{get_app_dir, SharedDirectory};
use iroh::SecretKey;
use iroh::discovery::UserData;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::Path;
use uuid::{Bytes, Uuid};

mod keyring;

const MAX_RENAME_ATTEMPS: u16 = 256;
const CONFIG_FILENAME: &str = "config.toml";

#[derive(Serialize, Deserialize)]
pub struct SyncifyConfigData {
    pub dirs: Vec<SharedDirectory>,
}

/// Config manager.
pub struct SyncifyConfig {
    pub secret_key: SecretKey,
    pub user_data: UserData,
    pub data: SyncifyConfigData,
}

impl Display for SyncifyConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(secret_key: {}, user_data: {})",
            self.secret_key, self.user_data
        )
    }
}

impl SyncifyConfig {
    pub async fn new() -> Result<Self, io::Error> {
        // Create app dir (and parents)
        if !get_app_dir().exists() {
            tokio::fs::create_dir_all(&get_app_dir()).await?;
        }

        let config_file = get_app_dir().join(CONFIG_FILENAME);

        // Create config file if it does not exist
        // Load the config file if it exists
        let syncify_config_data: SyncifyConfigData = {
            if Path::exists(config_file.as_path()) {
                info!("Reading config file...");
                let file_content: &mut String = &mut "".to_string();
                File::open(config_file.as_path())?.read_to_string(file_content)?;
                
                let result = toml::from_str(file_content);
                
                // Check if the config is parsable
                if let Ok(data) = result {
                    data
                } else {
                    warn!("Invalid config file: {}", config_file.display());
                    let mut i =0;
                    while std::fs::exists(config_file.join(format!(".backup{}", &i)).as_path())? {
                        i += 1;
                        if (i >= MAX_RENAME_ATTEMPS) {
                            break;
                        }
                    }
                    if i < MAX_RENAME_ATTEMPS {
                        std::fs::rename(config_file.as_path(), config_file.join(format!(".backup{}", &i)).as_path())?;
                    } else {
                        warn!("Unable backup config!");
                    }
                    
                    std::fs::remove_file(config_file.as_path())?;
                }
            }
            
            info!("Config file does not exists, creating a new one...");
            let mut w_file = File::create(config_file.as_path())?;
            let syncify_config_data = SyncifyConfigData { dirs: vec![] };

            w_file.write_all(toml::to_string(&syncify_config_data).unwrap().as_bytes())?;

            syncify_config_data
        };

        let syncify_keyring = SyncifyKeyring::new();
        let secret_key = {
            if !syncify_keyring.key_exists(Keys::SecretKey) {
                info!("Generating new secret key...");
                let mut rng = rand::rngs::OsRng;
                let key = SecretKey::generate(&mut rng);

                syncify_keyring
                    .set_key(Keys::SecretKey, key.to_string().as_str())
                    .unwrap();

                key
            } else {
                info!("Key already exists");
                syncify_keyring
                    .get_key(Keys::SecretKey)
                    .unwrap()
                    .parse()
                    .unwrap()
            }
        };

        Ok(SyncifyConfig {
            secret_key,
            user_data: "Philippe".parse().unwrap(),
            data: syncify_config_data,
        })
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

impl Display for SharedDirectory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.clone())
    }
}

impl Clone for SharedDirectory {
    fn clone(&self) -> Self {
        Self {
            uuid: self.uuid,
            path: self.path.clone(),
        }
    }
}

impl PartialEq<SharedDirectory> for SharedDirectory {
    fn eq(&self, other: &SharedDirectory) -> bool {
        self.uuid.eq(&other.uuid)
    }
}