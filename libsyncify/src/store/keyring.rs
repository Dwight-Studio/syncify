use keyring::{Credential, CredentialBuilder};
use std::thread;

pub enum Keys {
    SecretKey,
    SharedDirKey,
}

impl Keys {
    pub fn key_id<'a>(&self) -> &'a str {
        match self {
            Keys::SecretKey => "SecretKey",
            Keys::SharedDirKey => "SharedDirKey",
        }
    }
}

/// Secrets manager.
pub struct Keyring {
    credential_builder: Box<CredentialBuilder>,
}

impl Keyring {
    pub fn new() -> Self {
        Keyring {
            credential_builder: keyring::default::default_credential_builder(),
        }
    }

    pub fn set_key(&self, key: Keys, value: &str, key_complement: Option<&str>) -> Result<(), keyring::Error> {
        let credential = self.get_credential(key, key_complement);

        let val = value.to_string();

        thread::spawn(move || credential.set_password(val.as_str()))
            .join()
            .unwrap()
    }

    pub fn get_key(&self, key: Keys, key_complement: Option<&str>) -> Result<String, keyring::Error> {
        let credential = self.get_credential(key, key_complement);

        thread::spawn(move || credential.get_password())
            .join()
            .unwrap()
    }

    pub fn delete_key(&self, key: Keys, key_complement: Option<&str>) -> Result<(), keyring::Error> {
        let credential = self.get_credential(key, key_complement);

        thread::spawn(move || credential.delete_credential())
            .join()
            .unwrap()
    }

    pub fn key_exists(&self, key: Keys, key_complement: Option<&str>) -> bool {
        let credential = self.get_credential(key, key_complement);

        thread::spawn(move || credential.get_password())
            .join()
            .unwrap()
            .is_ok()
    }

    fn get_credential(&self, key: Keys, key_complement: Option<&str>) -> Box<Credential> {
        match key_complement {
            Some(key_comp) => {
                self.credential_builder
                    .build(
                        None,
                        (crate::APP_NAME.to_owned() + "-" + key.key_id() + "-" + key_comp).as_str(),
                        crate::APP_NAME,
                    )
                    .unwrap()
            }
            None => {
                self.credential_builder
                    .build(
                        None,
                        (crate::APP_NAME.to_owned() + "-" + key.key_id()).as_str(),
                        crate::APP_NAME,
                    )
                    .unwrap()
            }
        }
    }
}

/// Entity holding the shared folder secrets.
pub struct SharedDirectorySecrets {
    // TODO: Add the secrets
}