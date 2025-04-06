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
use crate::{SharedDirPermission, Syncify};
use base64::prelude::BASE64_STANDARD;
use base64::{DecodeError, Engine};
use rkyv::rancor::Error;
use rkyv::{Archive, Deserialize, Serialize};
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

/// The prefix used for invitation link generation
pub const LINK_PREFIX: &str = "syncify://";

/// The structure representing an invitation Link
#[derive(Debug, Clone, Archive, Serialize, Deserialize)]
pub struct Link {
    pub(crate) uuid: Uuid,
    pub(crate) permission: SharedDirPermission,
    pub(crate) key: [u8; 32],
    pub(crate) neighbors: Vec<[u8; 32]>,
}

impl Link {
    /// Get a builder to generate the invitation link.
    ///
    /// The default values are:
    ///  - permission = SharedDirPermission::Write
    ///  - uuid = Default of Uuid struct
    pub fn builder(syncify: Syncify) -> LinkBuilder {
        LinkBuilder {
            store: syncify.store,
            uuid: Uuid::default(),
            permission: SharedDirPermission::Write,
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
    type Err = LinkError;

    //noinspection RsTraitObligations
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let payload = if let Some(a) = s.split_at_checked(LINK_PREFIX.len()) {
            a.1
        } else {
            return Err(LinkError::MalformedLink);
        };

        if payload.is_empty() {
            return Err(LinkError::MalformedLink);
        }

        let decoded = BASE64_STANDARD.decode(payload).map_err(LinkError::Decoding)?;

        rkyv::from_bytes(decoded.as_slice()).map_err(LinkError::Deserializing)
    }
}

/// Builder structure to generate invitation links
pub struct LinkBuilder {
    store: Arc<RwLock<StoreManager>>,
    uuid: Uuid,
    permission: SharedDirPermission,
}

impl LinkBuilder {
    /// Build the invitation link
    pub async fn build(&self) -> Result<Link, LinkError> {
        let dir = self.store.read().await.get_shared_dir(&self.uuid);

        if let Some(dir) = dir {
            let key = {
                match self.permission {
                    SharedDirPermission::ReadOnly => dir.read_key.to_bytes(),
                    SharedDirPermission::Write => {
                        if let Some(key) = &dir.write_key {
                            key.to_bytes()
                        } else {
                            return Err(LinkError::InsufficientPermission);
                        }
                    }
                }
            };

            // Neighbors to list
            let mut neighbors = dir
                .neighbors
                .read()
                .await
                .iter()
                .map(|(node_id, _)| *node_id.as_bytes())
                .collect::<Vec<[u8; 32]>>();
            neighbors.insert(0, *self.store.read().await.public_key().as_bytes());

            Ok(Link {
                uuid: self.uuid,
                permission: self.permission.clone(),
                key,
                neighbors,
            })
        } else {
            Err(LinkError::UnknownDirectory)
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

#[derive(Error, Debug)]
pub enum LinkError {
    #[error("Malformed link")]
    MalformedLink,

    #[error("Cannot decode (of base64): {0}")]
    Decoding(DecodeError),

    #[error("Cannot decode (of base64): {0}")]
    Deserializing(Error),

    #[error("Insufficient permission (lacking write permission)")]
    InsufficientPermission,

    #[error("Unknown directory")]
    UnknownDirectory,
}
