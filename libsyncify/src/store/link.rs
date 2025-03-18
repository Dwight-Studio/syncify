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

use crate::store::StoreManager;
use crate::{SharedDirPermission, Syncify, SyncifyError};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use rkyv::rancor::Error;
use rkyv::{deserialize, Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// The prefix used for invitation link generation
const LINK_PREFIX: &str = "syncify://";

#[derive(Archive, Serialize, Deserialize)]
/// The structure representing an invitation Link 
pub struct Link {
    pub(crate) uuid: Uuid,
    pub(crate) permission: SharedDirPermission,
    pub(crate) key: [u8; 32],
    pub(crate) neighbors: HashMap<[u8; 32], bool>
}

impl Link {
    /// This function returns a builder to generate invitation link
    /// The default values are :
    ///  - permission = SharedDirPermission::Write
    ///  - uuid = Default of Uuid struct
    pub fn builder(syncify: Syncify) -> LinkBuilder {
        LinkBuilder {
            store: syncify.store,
            uuid: Uuid::default(),
            permission: SharedDirPermission::Write
        }
    }
}

impl Display for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let serialized = rkyv::to_bytes::<Error>(self).unwrap();
        
        write!(f, "{LINK_PREFIX}{}", BASE64_STANDARD.encode(serialized))
    }
}

impl FromStr for Link {
    type Err = SyncifyError;

    //noinspection RsTraitObligations
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let payload = s.split_at(LINK_PREFIX.len()).1;
        
        if payload.is_empty() { return Err(SyncifyError::LinkParseError(String::from("Invalid Link"))) }
        
        
        let decoded = BASE64_STANDARD.decode(payload).map_err(|e| SyncifyError::LinkParseError(e.to_string()))?;
        let archive: &ArchivedLink = rkyv::access::<ArchivedLink, Error>(decoded.as_slice()).map_err(|e| SyncifyError::LinkParseError(e.to_string()))?;
        let deserialize = deserialize::<Link, Error>(archive).map_err(|e| SyncifyError::LinkParseError(e.to_string()))?;
        
        Ok(deserialize)
    }
}

/// Builder structure to generate invitation links
pub struct LinkBuilder {
    store: Arc<RwLock<StoreManager>>,
    uuid: Uuid,
    permission: SharedDirPermission
}

impl LinkBuilder {
    /// Build the invitation link
    pub async fn build(&self) -> Result<Link, SyncifyError> {
        let dir = self.store.read().await.get_shared_dir(&self.uuid);

        if let Some(dir) = dir {
            let key = {
                match self.permission {
                    SharedDirPermission::ReadOnly => { dir.verif_key.to_bytes() }
                    SharedDirPermission::Write => {
                        if dir.sign_key.is_none() { return Err(SyncifyError::DirectoryReadOnly()) }
                        dir.sign_key.unwrap().to_bytes()
                    }
                }
            };

            let mut neighbors = dir.inner.read().await.neighbors.clone();
            neighbors.insert(*self.store.read().await.secret_key.public().as_bytes(), false);

            Ok(Link {
                uuid: self.uuid,
                permission: self.permission.clone(),
                key,
                neighbors
            })
        } else {
            Err(SyncifyError::DirectoryDoesNotExists(self.uuid))
        }
    }

    /// Sets the uuid that will be used in the invitation link
    pub fn dir_uuid(&mut self, uuid: Uuid) -> &mut Self {
        self.uuid = uuid;
        self
    }

    /// Sets the shared directory permission (SharedDirPermission::Write or SharedDirPermission::ReadOnly)
    pub fn permission(&mut self, permission: SharedDirPermission) -> &mut Self {
        self.permission = permission;
        self
    }
}
