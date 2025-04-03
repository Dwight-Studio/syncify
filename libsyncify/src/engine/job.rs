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
use blake3::Hash;
use chrono::{DateTime, Utc};
use iroh_base::NodeId;
use log::error;
use redb::{Key, TypeName, Value};
use rkyv::rancor::Error;
use rkyv::util::AlignedVec;
use rkyv::with::Skip;
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use uuid::Uuid;

/// A sync job.
#[derive(Archive, Serialize, Deserialize, Debug)]
pub struct DownloadJob {
    uuid: Uuid,
    path: String,
    #[rkyv(with = crate::util::HashDef)]
    hash: Hash,
    size: u64,
    #[rkyv(with = crate::util::DateTimeDef)]
    issued: DateTime<Utc>,
    pub(crate) state: JobState,
    pub progress: f32,
    pub(crate) last_chunk: u64,
    pub(crate) chunk_done: u64,
    pub(crate) failed_chunks: Vec<u64>,
}

impl DownloadJob {
    pub fn new(uuid: Uuid, path: String, hash: Hash, size: u64, issued: DateTime<Utc>, state: JobState) -> Self {
        Self {
            uuid,
            path,
            hash,
            size,
            issued,
            state,
            progress: 0f32,
            last_chunk: 0,
            chunk_done: 0,
            failed_chunks: Vec::new(),
        }
    }

    pub fn uuid(&self) -> &Uuid {
        &self.uuid
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn hash(&self) -> &Hash {
        &self.hash
    }

    pub fn size(&self) -> &u64 {
        &self.size
    }

    pub fn issued(&self) -> &DateTime<Utc> {
        &self.issued
    }

    pub fn state(&self) -> &JobState {
        &self.state
    }
    pub fn set_state(&mut self, state: JobState) {
        self.state = state;
    }
}

impl Value for DownloadJob {
    type SelfType<'a> = DownloadJob;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        None
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        rkyv::from_bytes::<DownloadJob, Error>(data).unwrap_or_else(|e| {
            error!("Failed to deserialize Download Job: {e}");
            DownloadJob::new(
                Uuid::default(),
                "Error".to_string(),
                Hash::from_bytes([0u8; 32]),
                0,
                Utc::now(),
                JobState::Error("Serialization".to_string()),
            )
        })
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'b,
    {
        rkyv::to_bytes(value)
            .unwrap_or_else(|e: Error| {
                error!("Failed to serialize download job: {e}");
                return AlignedVec::new();
            })
            .to_vec()
            .leak()
    }

    fn type_name() -> TypeName {
        TypeName::new("DownloadJob")
    }
}

#[derive(Archive, Serialize, Deserialize, Debug)]
pub enum JobState {
    Pending,
    Ongoing(#[rkyv(with = Skip)] Option<BufWriter<File>>),
    Done(#[rkyv(with = crate::util::DateTimeDef)] DateTime<Utc>),
    Error(String),
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct RemoteProvision {
    node: [u8; 32],
    #[rkyv(with = crate::util::HashDef)]
    hash: Hash,
    #[rkyv(with = crate::util::DateTimeDef)]
    expire: DateTime<Utc>,
}

impl Value for RemoteProvision {
    type SelfType<'a> = RemoteProvision;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        None
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        rkyv::from_bytes::<RemoteProvision, Error>(data).unwrap_or_else(|e| {
            error!("Failed to deserialize download job: {e}");
            RemoteProvision {
                node: [0; 32],
                hash: Hash::from_bytes([0u8; 32]),
                expire: Default::default(),
            }
        })
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'b,
    {
        rkyv::to_bytes(value)
            .unwrap_or_else(|e: rkyv::rancor::Error| {
                error!("Failed to serialize download job: {e}");
                AlignedVec::new()
            })
            .to_vec()
            .leak()
    }

    fn type_name() -> TypeName {
        TypeName::new("Provided")
    }
}

impl Key for RemoteProvision {
    //noinspection RsTraitObligations
    fn compare(data1_bytes: &[u8], data2_bytes: &[u8]) -> Ordering {
        if let (Ok(data1), Ok(data2)) = (
            rkyv::from_bytes::<RemoteProvision, Error>(data1_bytes),
            rkyv::from_bytes::<RemoteProvision, Error>(data2_bytes),
        ) {
            match data1.hash.as_bytes().cmp(data2.hash.as_bytes()) {
                Ordering::Equal => data1.node.cmp(&data2.node),
                ordering => ordering,
            }
        } else {
            Ordering::Greater
        }
    }
}

impl RemoteProvision {
    pub fn new(node_id: NodeId, hash: Hash, expiration: DateTime<Utc>) -> Self {
        Self {
            node: *node_id.as_bytes(),
            hash,
            expire: expiration,
        }
    }

    /// Check expiration.
    ///
    /// # Return
    ///
    /// Returns true if expired, false otherwise.
    pub fn is_expired(&self) -> bool {
        self.expire < Utc::now()
    }

    pub fn expiration(&self) -> DateTime<Utc> {
        self.expire
    }

    pub fn node_id(&self) -> Option<NodeId> {
        match NodeId::from_bytes(&self.node) {
            Ok(node_id) => Some(node_id),
            Err(_) => None,
        }
    }

    pub fn hash(&self) -> Hash {
        self.hash.clone()
    }
}

#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct LocalProvision {
    #[rkyv(with = crate::util::HashDef)]
    hash: Hash,
    #[rkyv(with = crate::util::DateTimeDef)]
    expire: DateTime<Utc>,
    path: String,
}

impl Value for LocalProvision {
    type SelfType<'a> = LocalProvision;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        None
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        rkyv::from_bytes::<LocalProvision, Error>(data).unwrap_or_else(|e| {
            error!("Failed to deserialize download job: {e}");
            LocalProvision {
                hash: Hash::from_bytes([0u8; 32]),
                expire: Default::default(),
                path: "".to_string(),
            }
        })
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'b,
    {
        rkyv::to_bytes(value)
            .unwrap_or_else(|e: rkyv::rancor::Error| {
                error!("Failed to serialize download job: {e}");
                AlignedVec::new()
            })
            .to_vec()
            .leak()
    }

    fn type_name() -> TypeName {
        TypeName::new("Provided")
    }
}

impl Key for LocalProvision {
    //noinspection RsTraitObligations
    fn compare(data1_bytes: &[u8], data2_bytes: &[u8]) -> Ordering {
        if let (Ok(data1), Ok(data2)) = (
            rkyv::from_bytes::<LocalProvision, Error>(data1_bytes),
            rkyv::from_bytes::<LocalProvision, Error>(data2_bytes),
        ) {
            data1.hash.as_bytes().cmp(data2.hash.as_bytes())
        } else {
            Ordering::Greater
        }
    }
}

impl LocalProvision {
    pub fn new(hash: Hash, expiration: DateTime<Utc>, path: PathBuf) -> Self {
        Self {
            hash,
            expire: expiration,
            path: path.to_string_lossy().to_string(),
        }
    }

    /// Check expiration.
    ///
    /// # Return
    ///
    /// Returns true if expired, false otherwise.
    pub fn is_expired(&self) -> bool {
        self.expire < Utc::now()
    }

    pub fn expiration(&self) -> DateTime<Utc> {
        self.expire.clone()
    }

    pub fn path(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    pub fn hash(&self) -> Hash {
        self.hash.clone()
    }
}
