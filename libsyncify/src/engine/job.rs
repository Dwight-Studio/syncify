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
use chrono::{DateTime, TimeDelta, Utc};
use iroh_base::NodeId;
use log::error;
use redb::{Key, TypeName, Value};
use rkyv::rancor::Error;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::Ordering;

/// A sync job.
#[derive(Archive, Serialize, Deserialize, Clone, Debug)]
pub struct DownloadJob {
    path: String,
    #[rkyv(with = crate::util::HashDef)]
    hash: Hash,
    #[rkyv(with = crate::util::DateTimeDef)]
    issued: DateTime<Utc>,
    state: JobState,
}

impl DownloadJob {
    pub fn new(path: String, hash: Hash, issued: DateTime<Utc>, state: JobState) -> Self {
        Self {
            path,
            hash,
            issued,
            state,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn hash(&self) -> &Hash {
        &self.hash
    }

    pub fn issued(&self) -> &DateTime<Utc> {
        &self.issued
    }

    pub fn state(&self) -> &JobState {
        &self.state
    }
}

impl Value for DownloadJob {
    type SelfType<'a> = DownloadJob;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        Option::from(size_of::<DownloadJob>())
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        rkyv::from_bytes::<DownloadJob, Error>(data).unwrap_or_else(|e| {
            error!("Failed to deserialize Download Job: {e}");
            return DownloadJob::new(
                "Error".to_string(),
                Hash::from_bytes([0u8; 32]),
                Utc::now(),
                JobState::Error("Serialization".to_string()),
            );
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

#[derive(Archive, Serialize, Deserialize, Clone, Debug)]
pub enum JobState {
    Pending,
    Ongoing(f32),
    Done(#[rkyv(with = crate::util::DateTimeDef)] DateTime<Utc>),
    Error(String),
}

#[derive(Archive, Serialize, Deserialize, Debug)]
pub struct Provision {
    node: [u8; 32],
    #[rkyv(with = crate::util::HashDef)]
    hash: Hash,
    #[rkyv(with = crate::util::DateTimeDef)]
    expire: DateTime<Utc>,
}

impl Value for Provision {
    type SelfType<'a> = Provision;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        Option::from(size_of::<Provision>())
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        rkyv::from_bytes::<Provision, Error>(data).unwrap_or_else(|e| {
            error!("Failed to deserialize download job: {e}");
            Provision {
                node: [0u8; 32],
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

impl Key for Provision {
    //noinspection RsTraitObligations
    fn compare(data1_bytes: &[u8], data2_bytes: &[u8]) -> Ordering {
        if let (Ok(data1), Ok(data2)) = (
            rkyv::from_bytes::<Provision, Error>(data1_bytes),
            rkyv::from_bytes::<Provision, Error>(data2_bytes),
        ) {
            match data1.hash.as_bytes().cmp(data2.hash.as_bytes()) {
                Ordering::Equal => data1.node.cmp(&data2.node),
                other => other,
            }
        } else {
            Ordering::Greater
        }
    }
}

impl Provision {
    pub fn remote(node_id: NodeId, hash: Hash, expiration: DateTime<Utc>) -> Self {
        Self {
            node: *node_id.as_bytes(),
            hash,
            expire: expiration,
        }
    }

    pub fn local(hash: Hash, expiration: DateTime<Utc>) -> Self {
        Self {
            node: [0u8; 32],
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
        self.expire.signed_duration_since(Utc::now()).le(&TimeDelta::zero())
    }

    pub fn expiration(&self) -> DateTime<Utc> {
        self.expire
    }

    pub fn node_id(&self) -> NodeId {
        NodeId::from_bytes(&self.node).unwrap()
    }

    pub fn hash(&self) -> Hash {
        self.hash
    }
}
