use blake3::Hash;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Serialize, Deserialize)]
#[serde(remote = "Hash")]
pub struct NewHash(#[serde(getter = "Hash::to_string")] String);

impl From<NewHash> for Hash {
    fn from(hash: NewHash) -> Self {
        Hash::from_str(hash.0.as_str()).unwrap()
    }
}

impl From<Hash> for NewHash {
    fn from(uuid: Hash) -> Self {
        NewHash(uuid.to_string())
    }
}
