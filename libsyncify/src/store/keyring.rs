use keyring::{Credential, CredentialBuilder};
use std::thread;

pub enum Keys {
    SecretKey,
}

impl Keys {
    pub fn key_id<'a>(&self) -> &'a str {
        match self {
            Keys::SecretKey => "SecretKey",
        }
    }
}

pub struct Keyring {
    credential_builder: Box<CredentialBuilder>,
}

impl Keyring {
    pub fn new() -> Self {
        Keyring {
            credential_builder: keyring::default::default_credential_builder(),
        }
    }

    pub fn set_key(&self, key: Keys, value: &str) -> Result<(), keyring::Error> {
        let credential = self.get_credential(key);

        let val = value.to_string();

        thread::spawn(move || credential.set_password(&val.as_str()))
            .join()
            .unwrap()
    }

    pub fn get_key(&self, key: Keys) -> Result<String, keyring::Error> {
        let credential = self.get_credential(key);

        thread::spawn(move || credential.get_password())
            .join()
            .unwrap()
    }

    pub fn delete_key(&self, key: Keys) -> Result<(), keyring::Error> {
        let credential = self.get_credential(key);

        thread::spawn(move || credential.delete_credential())
            .join()
            .unwrap()
    }

    pub fn key_exists(&self, key: Keys) -> bool {
        let credential = self.get_credential(key);

        thread::spawn(move || credential.get_password())
            .join()
            .unwrap()
            .is_ok()
    }

    fn get_credential(&self, key: Keys) -> Box<Credential> {
        self.credential_builder
            .build(
                None,
                (crate::APP_NAME.to_owned() + "-" + key.key_id()).as_str(),
                crate::APP_NAME,
            )
            .unwrap()
    }
}
