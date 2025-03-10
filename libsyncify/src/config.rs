mod keyring;

use std::fmt::{Display, Formatter};
use std::fs::File;
use iroh::SecretKey;
use iroh::discovery::UserData;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use crate::config::keyring::{Keys, SyncifyKeyring};

const CONFIG_FILENAME: &str = "config.toml";

#[derive(Serialize)]
#[derive(Deserialize)]
pub struct SyncifyFolder {
    pub name: String,
    pub path: String
}

#[derive(Serialize)]
#[derive(Deserialize)]
pub struct SyncifyConfigData {
    pub paths: Vec<SyncifyFolder>
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
    pub fn new() -> Result<Self, io::Error> {
        // Set the path where the configs file will be/is stored
        let config_dir: PathBuf = {
            if cfg!(debug_assertions) {
                PathBuf::from("./target/debug")
            } else {
                let project_dir =
                    directories::ProjectDirs::from("fr", "Dwight Studio", "Syncify").unwrap();
                project_dir.config_local_dir().to_path_buf()
            }
        };
        let config_file = config_dir.join(CONFIG_FILENAME);

        // Create config file if it does not exist
        // Load the config file if it exists
        let syncify_config_data: SyncifyConfigData = {
            if !Path::exists(config_file.as_path()) {
                info!("Config file does not exists, creating a new one...");
                let mut w_file = File::create(config_file.as_path())?;
                let syncify_config_data = SyncifyConfigData {paths: vec![]};

                w_file.write(toml::to_string(&syncify_config_data).unwrap().as_bytes())?;

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
