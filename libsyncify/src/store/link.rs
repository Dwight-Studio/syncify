use crate::store::StoreManager;
use crate::{SharedDirPermission, Syncify, SyncifyError};
use rkyv::rancor::Error;
use rkyv::{deserialize, Archive, Deserialize, Serialize};
use std::fmt::Display;
use std::str::FromStr;
use std::sync::Arc;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use tokio::sync::RwLock;
use uuid::{Bytes, Uuid};

const LINK_PREFIX: &str = "syncify://";

#[derive(Archive, Serialize, Deserialize)]
pub struct Link {
    #[rkyv(with = UuidDef)]
    uuid: Uuid,
    permission: SharedDirPermission,
    key: [u8; 32],
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
        if dir.is_none() { return Err(SyncifyError::DirectoryDoesNotExists(self.uuid)) }

        let key = {
            match self.permission {
                SharedDirPermission::ReadOnly => { dir.unwrap().verif_key.to_bytes() }
                SharedDirPermission::Write => {
                     if dir.clone().unwrap().sign_key.is_none() { return Err(SyncifyError::DirectoryReadOnly()) }
                    dir.unwrap().sign_key.unwrap().to_bytes()
                }
            }
        };

        Ok(Link {
            uuid: self.uuid,
            permission: self.permission.clone(),
            key
        })
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

#[derive(Archive, Serialize, Deserialize)]
#[rkyv(remote = uuid::Uuid)]
struct UuidDef(
    #[rkyv(getter = get_bytes)]
    Bytes
);

fn get_bytes(uuid: &Uuid) -> Bytes {
    uuid.into_bytes()
}

impl From<UuidDef> for Uuid {
    fn from(value: UuidDef) -> Self {
        Uuid::from_bytes(value.0)
    }
}