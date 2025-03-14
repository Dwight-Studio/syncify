use blake3::Hash;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(remote = "Uuid")]
pub struct UuidMock(#[serde(getter = "Uuid::to_string")] String);

impl From<UuidMock> for Uuid {
    fn from(uuid: UuidMock) -> Self {
        Uuid::from_str(uuid.0.as_str()).unwrap()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Hash")]
pub struct HashMock(#[serde(getter = "Hash::to_string")] String);

impl From<HashMock> for Hash {
    fn from(hash: HashMock) -> Self {
        Hash::from_str(hash.0.as_str()).unwrap()
    }
}
