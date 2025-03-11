use crate::config::keyring::{Keys, SyncifyKeyring};
use crate::get_app_dir;
use iroh::discovery::UserData;
use iroh::SecretKey;
use log::{debug, info};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::fs::File;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::{Bytes, Uuid};

mod keyring;

const CONFIG_FILENAME: &str = "config.toml";

#[derive(Serialize)]
#[derive(Deserialize)]
pub struct SyncifyFolder {
    #[serde(with = "UuidDef")]
    pub uuid: Uuid,
    
    pub path: String,
}

#[derive(Serialize)]
#[derive(Deserialize)]
pub struct SyncifyConfigData {
    pub folders: Vec<SyncifyFolder>
}

pub struct SyncifyConfig {
    pub secret_key: SecretKey,
    pub user_data: UserData,
    pub config_data: SyncifyConfigData
}

impl Display for SyncifyConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "(secret_key: {}, user_data: {})", self.secret_key, self.user_data)
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
            if !Path::exists(config_file.as_path()) {
                info!("Config file does not exists, creating a new one...");
                let mut w_file = File::create(config_file.as_path())?;
                let syncify_config_data = SyncifyConfigData { folders: vec![]};

                w_file.write_all(toml::to_string(&syncify_config_data).unwrap().as_bytes())?;

                syncify_config_data
            } else {
                info!("Reading config file...");
                let file_content: &mut String = &mut "".to_string();
                File::open(config_file.as_path())?.read_to_string(file_content)?;

                toml::from_str(file_content).unwrap()
            }
        };

        let syncify_keyring = SyncifyKeyring::new();
        let secret_key = {
            if !syncify_keyring.key_exists(Keys::SecretKey) {
                debug!("Generating new secret key...");
                let mut rng = rand::rngs::OsRng;
                let key = SecretKey::generate(&mut rng);

                syncify_keyring.set_key(Keys::SecretKey, key.to_string().as_str()).unwrap();

                key
            } else {
                debug!("Key already exists");
                syncify_keyring.get_key(Keys::SecretKey).unwrap().parse().unwrap()
            }
        };

        Ok(SyncifyConfig{secret_key, user_data: "Philippe".parse().unwrap(), config_data: syncify_config_data})
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Uuid")]
pub struct UuidDef(
    #[serde(getter = "Uuid::as_bytes")]
    Bytes
);

impl From<UuidDef> for Uuid {
    fn from(uuid: UuidDef) -> Self {
        Uuid::from_bytes(uuid.0)
    }
}