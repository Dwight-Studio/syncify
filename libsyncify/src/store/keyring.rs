/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

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
#[derive(Debug)]
pub struct Keyring {
    credential_builder: Box<CredentialBuilder>,
}

impl Default for Keyring {
    fn default() -> Self {
        Self::new()
    }
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

        thread::spawn(move || credential.get_password()).join().unwrap()
    }

    pub fn delete_key(&self, key: Keys, key_complement: Option<&str>) -> Result<(), keyring::Error> {
        let credential = self.get_credential(key, key_complement);

        thread::spawn(move || credential.delete_credential()).join().unwrap()
    }

    pub fn key_exists(&self, key: Keys, key_complement: Option<&str>) -> bool {
        let credential = self.get_credential(key, key_complement);

        thread::spawn(move || credential.get_password()).join().unwrap().is_ok()
    }

    fn get_credential(&self, key: Keys, key_complement: Option<&str>) -> Box<Credential> {
        match key_complement {
            Some(key_comp) => self
                .credential_builder
                .build(
                    None,
                    (crate::APP_NAME.to_owned() + "-" + key.key_id() + "-" + key_comp).as_str(),
                    crate::APP_NAME,
                )
                .unwrap(),
            None => self
                .credential_builder
                .build(
                    None,
                    (crate::APP_NAME.to_owned() + "-" + key.key_id()).as_str(),
                    crate::APP_NAME,
                )
                .unwrap(),
        }
    }
}
