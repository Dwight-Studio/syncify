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

use crate::engine::state::{Delta, HashTree, Mutation, State, MAX_LOADED_DELTAS};
use blake3::Hash;
use chrono::{DateTime, Utc};
use log::error;
use redb::{ReadableTable, Table};
use rkyv::rancor::Error;
use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Serializable form of [`State`].
#[derive(Archive, Serialize, Deserialize, Debug)]
pub struct SerialState {
    head: [u8; 32],
    pool: HashMap<[u8; 32], SerialDelta>,
    timestamp: i64,
}

impl SerialState {
    pub fn new(head: [u8; 32], pool: HashMap<[u8; 32], SerialDelta>, timestamp: i64) -> Self {
        Self {
            head,
            pool,
            timestamp,
        }
    }

    pub fn head(&self) -> [u8; 32] {
        self.head.clone()
    }

    pub fn pool(self) -> HashMap<[u8; 32], SerialDelta> {
        self.pool
    }

    //noinspection RsTraitObligations
    pub fn build_from_table(
        state_table: Table<[u8; 32], &[u8]>,
        head_hash: [u8; 32],
    ) -> Option<State> {
        if let Ok(Some(head_access)) = state_table.get(&head_hash) {
            match rkyv::from_bytes::<SerialDelta, Error>(head_access.value()) {
                Ok(head) => {
                    let mut pool = HashMap::new();

                    // Insert head into the pool
                    pool.insert(head_hash, head.clone());

                    let mut parent_hash = head.parent;

                    for _ in 0..MAX_LOADED_DELTAS {
                        if let Ok(Some(parent_access)) = state_table.get(&parent_hash) {
                            match rkyv::from_bytes::<SerialDelta, Error>(parent_access.value()) {
                                Ok(parent) => {
                                    pool.insert(parent_hash, parent.clone());

                                    // Check if it reached the root
                                    if parent.hash != parent.parent {
                                        parent_hash = parent.parent;
                                    } else {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    error!("Could not deserialize delta {} ({})", Hash::from_bytes(parent_hash), e);
                                    return None;
                                }
                            };
                        } else {
                            break;
                        }
                    }

                    if pool.len() != 0 {
                        Some(State::from(SerialState::new(
                            head_hash,
                            pool,
                            Utc::now().timestamp(),
                        )))
                    } else {
                        None
                    }
                }
                Err(e) => {
                    error!("Could not deserialize head {} ({})", Hash::from_bytes(head_hash), e);
                    None
                }
            }
        } else {
            None
        }
    }
}

impl From<&State> for SerialState {
    fn from(value: &State) -> Self {
        let mut pool: HashMap<[u8; 32], SerialDelta> = HashMap::new();

        for head_ref in value.iter() {
            let head = head_ref.read().unwrap();

            if head.parent.is_none() {
                // If so, break
                pool.insert(
                    *head.hash.as_bytes(),
                    SerialDelta {
                        parent: *head.hash.as_bytes(),
                        hash: *head.hash.as_bytes(),
                        hash_tree_cache: head.hash_tree_cache.clone(),
                        action: head.action.clone(),
                        timestamp: head.timestamp.timestamp(),
                    },
                );
                break;
            } else {
                // If not, continue
                pool.insert(
                    *head.hash.as_bytes(),
                    SerialDelta {
                        parent: *head.parent.clone().unwrap().read().unwrap().hash.as_bytes(),
                        hash: *head.hash.as_bytes(),
                        hash_tree_cache: head.hash_tree_cache.clone(),
                        action: head.action.clone(),
                        timestamp: head.timestamp.timestamp(),
                    },
                );
            }
        }

        Self {
            head: *value.head.read().unwrap().hash.as_bytes(),
            pool,
            timestamp: value.timestamp.timestamp(),
        }
    }
}

impl From<SerialState> for State {
    fn from(value: SerialState) -> Self {
        State {
            head: Arc::new(RwLock::new(
                from_recursive(value.head, &value.pool).unwrap(),
            )),
            timestamp: DateTime::<Utc>::from_timestamp(value.timestamp, 0).unwrap(),
        }
    }
}

fn from_recursive(head_hash: [u8; 32], pool: &HashMap<[u8; 32], SerialDelta>) -> Option<Delta> {
    let head = pool.get(&head_hash)?;

    if head.hash != head.parent {
        let opt_delta = from_recursive(head.parent, pool);

        if let Some(delta) = opt_delta {
            return Some(Delta {
                parent: Some(Arc::new(RwLock::new(delta))),
                hash: Hash::from_bytes(head.hash),
                hash_tree_cache: head.hash_tree_cache.clone(),
                action: head.action.clone(),
                timestamp: DateTime::<Utc>::from_timestamp(head.timestamp, 0).unwrap(),
            });
        }
    }

    Some(Delta {
        parent: None,
        hash: Hash::from_bytes(head.hash),
        hash_tree_cache: head.hash_tree_cache.clone(),
        action: head.action.clone(),
        timestamp: DateTime::<Utc>::from_timestamp(head.timestamp, 0).unwrap(),
    })
}

/// Serializable form of [`Delta`].
#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct SerialDelta {
    parent: [u8; 32],
    hash: [u8; 32],
    hash_tree_cache: Option<HashTree>,
    action: Mutation,
    timestamp: i64,
}
