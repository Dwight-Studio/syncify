use crate::engine::state::{Delta, HashTree, Mutation, State, MAX_LOADED_DELTAS};
use blake3::Hash;
use redb::{ReadableTable, Table, TypeName, Value};
use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Archive, Serialize, Deserialize, Debug)]
pub struct SerialState {
    head: [u8; 32],
    pool: HashMap<[u8; 32], SerialDelta>,
}

impl SerialState {
    pub fn new(head: [u8; 32], pool: HashMap<[u8; 32], SerialDelta>) -> Self {
        Self { head, pool }
    }

    pub fn head(&self) -> [u8; 32] {
        self.head.clone()
    }

    pub fn pool(self) -> HashMap<[u8; 32], SerialDelta> {
        self.pool
    }
}

impl Value for SerialState {
    type SelfType<'a> = SerialState;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        None
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        let archived = rkyv::access::<ArchivedSerialState, rkyv::rancor::Error>(data).unwrap();
        rkyv::deserialize::<SerialState, rkyv::rancor::Error>(archived).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'b,
    {
        rkyv::to_bytes::<rkyv::rancor::Error>(value)
            .unwrap()
            .to_vec()
            .leak()
    }

    fn type_name() -> TypeName {
        TypeName::new("SerialState")
    }
}

impl From<&State> for SerialState {
    fn from(value: &State) -> Self {
        let mut pool: HashMap<[u8; 32], SerialDelta> = HashMap::new();

        let mut head_ref = value.head.clone();

        loop {
            let head = head_ref.read().unwrap();
            let parent = head.parent.clone();

            // Check if it reached the root
            if parent.is_none() {
                // If so, break
                pool.insert(
                    *head.hash.as_bytes(),
                    SerialDelta {
                        parent: *head.hash.as_bytes(),
                        hash: *head.hash.as_bytes(),
                        hash_tree_cache: head.hash_tree_cache.clone(),
                        action: head.action.clone(),
                    },
                );
                break;
            } else {
                // If not, continue
                pool.insert(
                    *head.hash.as_bytes(),
                    SerialDelta {
                        parent: *parent.clone().unwrap().read().unwrap().hash.as_bytes(),
                        hash: *head.hash.as_bytes(),
                        hash_tree_cache: head.hash_tree_cache.clone(),
                        action: head.action.clone(),
                    },
                );
            }

            // Drop head to be able to use borrow head_ref
            drop(head);

            head_ref = parent.clone().unwrap();
        }

        Self {
            head: *value.head.read().unwrap().hash.as_bytes(),
            pool,
        }
    }
}

impl From<SerialState> for State {
    fn from(value: SerialState) -> Self {
        State {
            head: Arc::new(RwLock::new(from_recursive(value.head, &value.pool).unwrap())),
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
            });
        }
    }

    Some(Delta {
        parent: None,
        hash: Hash::from_bytes(head.hash),
        hash_tree_cache: head.hash_tree_cache.clone(),
        action: head.action.clone(),
    })
}

pub fn build_serial_state(state_table: Table<[u8; 32], SerialDelta>, head_hash: [u8; 32]) -> Option<State> {
    if let Ok(Some(head_access)) = state_table.get(&head_hash) {
        let head = head_access.value();
        let mut pool = HashMap::new();

        // Insert head into the pool
        pool.insert(head_hash, head.clone());

        let mut parent_hash = head.parent;

        for _ in 0..MAX_LOADED_DELTAS {
            if let Ok(Some(parent_access)) = state_table.get(&parent_hash) {
                let parent = parent_access.value();
                pool.insert(parent_hash, parent.clone());

                // Check if it reached the root
                if parent.hash != parent.parent {
                    parent_hash = parent.parent;
                } else {
                    break
                }
            } else {
                break
            }
        }

        if pool.len() != 0 {
            Some(State::from(SerialState::new(head_hash, pool)))
        } else {
            None
        }
    } else {
        None
    }
}


#[derive(Archive, Serialize, Deserialize, Debug, Clone)]
pub struct SerialDelta {
    parent: [u8; 32],
    hash: [u8; 32],
    hash_tree_cache: Option<HashTree>,
    action: Mutation,
}

impl Value for SerialDelta {
    type SelfType<'a> = SerialDelta;
    type AsBytes<'a> = &'a [u8];

    fn fixed_width() -> Option<usize> {
        None
    }

    //noinspection RsTraitObligations
    fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
    where
        Self: 'a,
    {
        let archived = rkyv::access::<ArchivedSerialDelta, rkyv::rancor::Error>(data).unwrap();
        rkyv::deserialize::<SerialDelta, rkyv::rancor::Error>(archived).unwrap()
    }

    fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
    where
        Self: 'b,
    {
        rkyv::to_bytes::<rkyv::rancor::Error>(value)
            .unwrap()
            .to_vec()
            .leak()
    }

    fn type_name() -> TypeName {
        TypeName::new("SerialDelta")
    }
}