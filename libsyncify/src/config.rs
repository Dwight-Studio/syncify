use std::fmt::{Display, Formatter};
use std::fs::File;
use iroh::SecretKey;
use iroh::discovery::UserData;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeStruct;

const CONFIG_FILENAME: &str = "config.toml";

// TODO: Might need more work to simplify Deserialize & Serialize implementation
pub struct SyncifyConfig {
    pub secret_key: SecretKey,
    pub user_data: UserData
}

impl Display for SyncifyConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "(secret_key: {}, user_data: {})", self.secret_key, self.user_data)
    }
}

impl Serialize for SyncifyConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer
    {
        let mut s = serializer.serialize_struct("SyncifyConfig", 2)?;
        s.serialize_field("secretkey", &self.secret_key.to_string())?;
        s.serialize_field("userdata", &self.user_data.to_string())?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for SyncifyConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>
    {
        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field { SecretKey, UserData }

        struct SyncifyConfigVisitor;

        impl<'de> Visitor<'de> for SyncifyConfigVisitor {
            type Value = SyncifyConfig;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                formatter.write_str("struct SyncifyConfig")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>
            {
                let raw_secret_key: String = seq.next_element()?.ok_or_else(|| Error::invalid_length(0, &self))?;
                let raw_user_data: String = seq.next_element()?.ok_or_else(|| Error::invalid_length(1, &self))?;

                let secret_key = raw_secret_key.parse().unwrap();
                let user_data = raw_user_data.parse().unwrap();

                Ok(SyncifyConfig{secret_key, user_data})
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>
            {
                let mut raw_secret_key = None;
                let mut raw_user_data = None;

                while let Some(key) = map.next_key()? {
                    match key {
                        Field::SecretKey => {
                            if raw_secret_key.is_some() {
                                return Err(Error::duplicate_field("secretkey"));
                            }
                            raw_secret_key = Some(map.next_value()?);
                        }
                        Field::UserData => {
                            if raw_user_data.is_some() {
                                return Err(Error::duplicate_field("userdata"));
                            }
                            raw_user_data = Some(map.next_value()?);
                        }
                    }
                }
                let raw_secret_key: String = raw_secret_key.ok_or_else(|| Error::missing_field("secretkey"))?;
                let raw_user_data: String = raw_user_data.ok_or_else(|| Error::missing_field("userdata"))?;

                let secret_key = raw_secret_key.parse().unwrap();
                let user_data = raw_user_data.parse().unwrap();

                Ok(SyncifyConfig{secret_key, user_data})
            }
        }

        const FIELDS: &[&str] = &["secretkey", "userdata"];
        deserializer.deserialize_struct("SyncifyConfig", FIELDS, SyncifyConfigVisitor)
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
        if !Path::exists(config_file.as_path()) {
            println!("Config file does not exists, creating a new one...");
            let mut w_file = File::create(config_file.as_path())?;
            let mut rng = rand::rngs::OsRng; // TODO: This must be changed: it does not work with the latest rand crate version
            let secret_key = SecretKey::generate(&mut rng);
            let user_data: UserData = "Philippe".parse().unwrap(); //TODO: UserData should be chosen by the user himself
            let syncify_config = SyncifyConfig{secret_key, user_data};

            w_file.write(toml::to_string(&syncify_config).unwrap().as_bytes())?;

            Ok(syncify_config)
        } else {
            println!("Reading config file...");
            let file_content: &mut String = &mut "".to_string();
            File::open(config_file.as_path())?.read_to_string(file_content)?;

            Ok(toml::from_str(file_content).unwrap())
        }
    }
}
