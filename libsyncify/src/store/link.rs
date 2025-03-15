use crate::store::StoreManager;
use crate::{SharedDirPermission, Syncify, SyncifyError};
use rkyv::rancor::Error;
use rkyv::{Archive, Deserialize, Serialize};
use std::fmt::Display;
use std::sync::Arc;
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use tokio::sync::RwLock;
use uuid::{Bytes, Uuid};

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
        
        write!(f, "syncify://{}", BASE64_STANDARD.encode(serialized))
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