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

const LINK_PREFIX: &str = "syncify://";

#[derive(Archive, Serialize, Deserialize)]
pub struct Link {
    pub(crate) uuid: Uuid,
    pub(crate) permission: SharedDirPermission,
    pub(crate) key: [u8; 32],
    pub(crate) neighbors: HashMap<[u8; 32], bool>
}

impl Link {
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

pub struct LinkBuilder {
    store: Arc<RwLock<StoreManager>>,
    uuid: Uuid,
    permission: SharedDirPermission
}

impl LinkBuilder {
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

    pub fn dir_uuid(&mut self, uuid: Uuid) -> &mut Self {
        self.uuid = uuid;
        self
    }

    pub fn permission(&mut self, permission: SharedDirPermission) -> &mut Self {
        self.permission = permission;
        self
    }
}
