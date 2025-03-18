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

use crate::SharedDirectory;
use crate::engine::state::HashTree::{Directory, File, Void};
use crate::engine::state::StateError::NotADirectory;
use blake3::Hash;
use chrono::{DateTime, Utc};
use log::{error, warn};
use redb::{ReadableTable, Table, Value};
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::iter::Peekable;
use std::sync::{Arc};
use ed25519_dalek::{Signature, Signer, SigningKey};
use ed25519_dalek::ed25519::SignatureBytes;
use thiserror::Error;
use uuid::Uuid;
use walkdir::WalkDir;

pub const MAX_LOADED_DELTAS: u32 = 2048;
pub const MAX_UNFLUSHED_DELTAS: u32 = MAX_LOADED_DELTAS * 32;

/// Tree containing the synchronisation information for a [`SharedDirectory`].
#[derive(Archive, Serialize, Deserialize, Clone, Debug)]
pub struct State {
    #[rkyv(with = crate::util::HashDef)]
    head: Hash,
    pool: HashMap<[u8; 32], Arc<Delta>>,
    /// The timestamp is dated from last time it was "seen" out of the cache
    /// i.e. last time it was saved, loaded or synced.
    #[rkyv(with = crate::util::DateTimeDef)]
    timestamp: DateTime<Utc>,
}

// TODO: Add optimization
//  -> Compute when last modified date is > than the last save date (for fastforward sync)
impl State {
    pub fn new(uuid: Uuid) -> Self {
        let timestamp = Utc::now();
        let mut pool = HashMap::new();
        let hash = blake3::hash(uuid.as_bytes());

        pool.insert(
            *hash.as_bytes(),
            Arc::new(Delta {
                parent: None,
                hash,
                signature: Signature::from_bytes(&SignatureBytes::from_bytes(&[0u8; 64])),
                timestamp,
                action: Mutation::Init {
                    timestamp: Utc::now(),
                },
                hash_tree: Directory {
                    name: uuid.to_string(),
                    content: vec![],
                    hash: Hash::from_bytes([0; 32]),
                },
            }),
        );

        Self {
            head: hash,
            pool,
            timestamp,
        }
    }

    //noinspection RsTraitObligations
    pub fn from_table(state_table: Table<[u8; 32], &[u8]>, head_hash: [u8; 32]) -> Option<State> {
        if let Ok(Some(head_access)) = state_table.get(&head_hash) {
            match rkyv::from_bytes::<Delta, rkyv::rancor::Error>(head_access.value()) {
                Ok(head) => {
                    let mut pool = HashMap::new();

                    let mut parent_opt = head.parent;

                    // Insert head into the pool
                    pool.insert(head_hash, Arc::new(head));

                    for _ in 0..MAX_LOADED_DELTAS {
                        if let Some(parent_hash) = parent_opt {
                            if let Ok(Some(parent_access)) = state_table.get(parent_hash.as_bytes())
                            {
                                match rkyv::from_bytes::<Delta, rkyv::rancor::Error>(
                                    parent_access.value(),
                                ) {
                                    Ok(parent) => {
                                        parent_opt = parent.parent;
                                        pool.insert(*parent_hash.as_bytes(), Arc::new(parent));
                                    }
                                    Err(e) => {
                                        error!(
                                            "Could not deserialize delta {} ({})",
                                            parent_hash, e
                                        );
                                        return None;
                                    }
                                };
                            } else {
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    if pool.len() != 0 {
                        Some(State {
                            head: Hash::from_bytes(head_hash),
                            pool,
                            timestamp: Utc::now(),
                        })
                    } else {
                        None
                    }
                }
                Err(e) => {
                    error!(
                        "Could not deserialize head {} ({})",
                        Hash::from_bytes(head_hash),
                        e
                    );
                    None
                }
            }
        } else {
            error!("Unable to find head {}", Hash::from_bytes(head_hash));
            None
        }
    }

    /// Get head's [`Delta`].
    pub fn head(&self) -> &Arc<Delta> {
        self.get(&self.head).unwrap()
    }

    pub fn pool(&self) -> &HashMap<[u8; 32], Arc<Delta>> {
        &self.pool
    }

    /// Get a [`Delta`].
    fn get(&self, hash: &Hash) -> Option<&Arc<Delta>> {
        self.pool.get(hash.as_bytes())
    }

    /// Get parent [`State`].
    pub fn parent(&self) -> Option<Self> {
        let head = self.head();
        if let Some(parent_hash) = head.parent {
            self.get(&parent_hash).map(|parent| Self {
                head: parent.hash,
                pool: self.pool.clone(),
                timestamp: self.timestamp,
            })
        } else {
            None
        }
    }

    /// Get head's hash.
    pub fn hash(&self) -> Hash {
        self.head().hash
    }

    /// Get [`State`]'s files hash tree.
    pub fn hash_tree(&self) -> &HashTree {
        &self.head().hash_tree
    }

    /// Apply a mutation on the [`State`].
    pub fn mutate(&mut self, mutation: Mutation, write_key: &SigningKey) -> Result<(), StateError> {
        let mut delta = Delta {
            parent: Some(self.head.clone()),
            hash: Hash::from_bytes([0; 32]),
            signature: Signature::from_bytes(&SignatureBytes::from_bytes(&[0u8; 64])),
            timestamp: Utc::now(),
            action: mutation.clone(),
            hash_tree: self.head().hash_tree.apply(&mutation)?,
        };

        // Compute hash
        delta.hash = {
            let mut data = Vec::new();

            // Parent
            data.extend(self.head.as_bytes());

            // Tree
            data.extend(delta.hash_tree.hash().as_bytes());
            
            blake3::hash(data.leak())
        };
        
        // Compute signature
        delta.signature = {
            let mut data = Vec::new();
            
            // Hash
            data.extend(delta.hash.as_bytes());
            
            // Parent
            data.extend(self.head.as_bytes());
            
            // Tree
            data.extend(delta.hash_tree.hash().as_bytes());
            
            write_key.sign(data.leak())
        };

        // Modify head and insert into pool
        self.head = delta.hash;
        self.pool.insert(*delta.hash.as_bytes(), Arc::new(delta));

        Ok(())
    }

    /// Trim [`State`]'s tree of all deltas over the limit of loaded deltas.
    ///
    /// Return true if the State was pruned.
    pub fn trim(&mut self) -> bool {
        if self.pool.len() > MAX_LOADED_DELTAS as usize {
            let mut new_pool = HashMap::new();

            let mut head = self.head();

            for i in 0..MAX_LOADED_DELTAS {
                new_pool.insert(head.hash, head.clone());

                if let Some(parent_hash) = head.parent {
                    if let Some(parent) = self.get(&parent_hash) {
                        head = parent
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }

            let mut new_head = head.as_ref().clone();
            new_head.parent = None;
            new_pool.insert(new_head.hash, Arc::new(new_head));

            true
        } else {
            false
        }
    }

    pub fn iter(&self) -> StateIterator {
        StateIterator {
            pool: self.pool.clone(),
            head_opt: Some(self.head().clone()),
        }
    }

    pub fn clone_after(&self, root: Hash, max_depth: u32) -> Option<State> {
        let mut pool = HashMap::new();
        let mut head = self.head();

        for i in 0..max_depth {
            // Check if we reached the root
            if self.head == root {
                let mut delta = head.as_ref().clone();
                delta.parent = None;
                pool.insert(*head.hash.as_bytes(), Arc::new(delta));

                return Some(State {
                    head: self.head,
                    pool,
                    timestamp: self.timestamp,
                });
            } else {
                pool.insert(*head.hash.as_bytes(), head.clone());

                if let Some(parent_hash) = head.parent {
                    if let Some(parent) = self.get(&parent_hash) {
                        head = parent
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        None
    }
}

impl Display for State {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        writeln!(f, "State")?;
        writeln!(f, "Timestamp: {}", self.timestamp)?;
        writeln!(f)?;
        writeln!(f, "■ Current")?;

        for head in self.iter() {
            let mut prefix = "│";
            writeln!(f, "{prefix}")?;

            if head.parent.is_some() {
                writeln!(f, "├─ {}", head.hash)?;
            } else {
                writeln!(f, "└─ {}", head.hash)?;
                prefix = " ";
            }

            writeln!(f, "{prefix}  Timestamp: {}", head.timestamp)?;
            writeln!(f, "{prefix}  {}", head.action)?;
        }

        Ok(())
    }
}

pub struct StateIterator {
    pool: HashMap<[u8; 32], Arc<Delta>>,
    head_opt: Option<Arc<Delta>>,
}

impl Iterator for StateIterator {
    type Item = Arc<Delta>;

    fn next(&mut self) -> Option<Self::Item> {
        match &self.head_opt {
            Some(head) => {
                let rtn = head.clone();

                self.head_opt = match head.parent {
                    Some(parent) => self.pool.get(parent.as_bytes()).cloned(),
                    None => None,
                };

                Some(rtn)
            }
            None => None,
        }
    }
}

/// Node describing a modification of a [`State`].
#[derive(Archive, Serialize, Deserialize, Clone, Debug)]
pub struct Delta {
    #[rkyv(with = crate::util::OptionHashDef)]
    parent: Option<Hash>,
    #[rkyv(with = crate::util::HashDef)]
    hash: Hash,
    #[rkyv(with = crate::util::SignatureDef)]
    signature: Signature,
    /// Timestamp is dated from when the mutation was applied.
    #[rkyv(with = crate::util::DateTimeDef)]
    timestamp: DateTime<Utc>,
    action: Mutation,
    hash_tree: HashTree,
}

/// Mutation action of a [`Delta`].
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum Mutation {
    Init {
        /// Timestamp is dated from when the mutation was detected.
        #[rkyv(with = crate::util::DateTimeDef)]
        timestamp: DateTime<Utc>,
    },
    Merge {
        #[rkyv(with = crate::util::HashDef)]
        other_head: Hash,
        /// Timestamp is dated from when the mutation was detected.
        #[rkyv(with = crate::util::DateTimeDef)]
        timestamp: DateTime<Utc>,
    },
    Modify {
        file_path: String,
        #[rkyv(with = crate::util::HashDef)]
        file_hash: Hash,
        /// Timestamp is dated from when the mutation was detected.
        #[rkyv(with = crate::util::DateTimeDef)]
        timestamp: DateTime<Utc>,
    },
    Move {
        from: String,
        to: String,
        /// Timestamp is dated from when the mutation was detected.
        #[rkyv(with = crate::util::DateTimeDef)]
        timestamp: DateTime<Utc>,
    },
    Remove {
        file_path: String,
        /// Timestamp is dated from when the mutation was detected.
        #[rkyv(with = crate::util::DateTimeDef)]
        timestamp: DateTime<Utc>,
    },
}

impl Display for Mutation {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            Mutation::Init { .. } => write!(f, "Initialized directory"),
            Mutation::Merge { other_head, .. } => {
                write!(f, "Merged branch {}", *other_head)
            }
            Mutation::Modify { file_path, .. } => write!(f, "Modified file \"{}\"", file_path),
            Mutation::Move { from, to, .. } => write!(f, "Moved file \"{from}\" to \"{to}\""),
            Mutation::Remove { file_path, .. } => write!(f, "Removed file \"{}\"", file_path),
        }
    }
}

/// Hash tree describing a state of the file tree.
#[derive(Clone, Debug, Eq, Archive, Serialize, Deserialize)]
#[rkyv(serialize_bounds(
            __S: rkyv::ser::Writer + rkyv::ser::Allocator,
            __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(
    bounds(
        __C: rkyv::validation::ArchiveContext,
    )
))]
pub enum HashTree {
    Void,
    File {
        name: String,
        #[rkyv(with = crate::util::HashDef)]
        hash: Hash,
        #[rkyv(with = crate::util::DateTimeDef)]
        timestamp: DateTime<Utc>,
    },
    Directory {
        name: String,
        #[rkyv(omit_bounds)]
        content: Vec<HashTree>,
        #[rkyv(with = crate::util::HashDef)]
        hash: Hash,
    },
}

impl HashTree {
    /// Get a reference to the hash tree at the path.
    pub fn get(&self, file_path: &str) -> Option<&HashTree> {
        self.get_recursive(&mut file_path.split("/").peekable())
    }

    fn get_recursive<'a, A>(&self, file_path_iter: &mut Peekable<A>) -> Option<&HashTree>
    where
        A: Iterator<Item = &'a str> + Clone,
    {
        match &self {
            Void => None,
            File { .. } => Some(self),
            Directory { content, .. } => {
                if let Some(path) = file_path_iter.next() {
                    for tree in content.iter() {
                        match tree {
                            Void => {}
                            File { name, .. } | Directory { name, .. } => {
                                if name == path {
                                    return tree.get_recursive(file_path_iter);
                                }
                            }
                        }
                    }
                } else {
                    return Some(self)
                }

                None
            }
        }
    }

    /// Construct a mutated version of the [`HashTree`].
    fn apply(&self, mutation: &Mutation) -> Result<HashTree, StateError> {
        match mutation {
            Mutation::Init { .. } => Ok(self.clone()),
            Mutation::Merge { .. } => Ok(self.clone()),
            Mutation::Modify {
                file_path,
                file_hash,
                ..
            } => Self::apply_and_update_parents(
                self.clone(),
                &mut |_: HashTree| -> HashTree {
                    File {
                        name: file_path.split("/").last().unwrap().to_string(),
                        hash: *file_hash,
                        timestamp: Utc::now(),
                    }
                },
                file_path.split("/").peekable(),
            ),
            Mutation::Move { from, to, .. } => {
                let extracted = self.get(from).unwrap();
                let tree = Self::apply_and_update_parents(
                    self.clone(),
                    &mut |_: HashTree| -> HashTree { Void },
                    from.split("/").peekable(),
                )?;
                Self::apply_and_update_parents(
                    tree,
                    &mut |_: HashTree| -> HashTree { extracted.clone() },
                    to.split("/").peekable(),
                )
            }
            Mutation::Remove { file_path, .. } => Self::apply_and_update_parents(
                self.clone(),
                &mut |_: HashTree| -> HashTree { Void },
                file_path.split("/").peekable(),
            ),
        }
    }

    /// Apply a function on a file/folder provided with path, and update parents' hash.
    fn apply_and_update_parents<'a>(
        parent: HashTree,
        mut_fn: &mut dyn FnMut(HashTree) -> HashTree,
        mut path: Peekable<impl Iterator<Item = &'a str>>,
    ) -> Result<HashTree, StateError> {
        let next = path.next();
        let has_next = path.peek().is_some();

        // Check if it reaches the end of the iterator
        if let Some(elem) = next {
            // If not, check if the current parent is a directory
            let rtn = match parent {
                Directory {
                    name,
                    content,
                    hash,
                } => {
                    // If so, construct new content by searching for the elem
                    let mut new_content: Vec<HashTree> = Vec::new();
                    let mut elem_pos: i64 = -1;
                    for (pos, tree) in content.into_iter().enumerate() {
                        match &tree {
                            Void => {}
                            File { name, .. } | Directory { name, .. } => {
                                if name == elem {
                                    elem_pos = pos as i64;
                                }
                            }
                        }
                        new_content.push(tree);
                    }

                    // Check if the parent contains the next elem
                    let result = if elem_pos == -1 {
                        // If not, let the function construct the object
                        let new_tree = if has_next {
                            Directory {
                                name: elem.to_string(),
                                content: Vec::new(),
                                hash: Hash::from_bytes([0; 32]),
                            }
                        } else {
                            File {
                                name: elem.to_string(),
                                hash: Hash::from_bytes([0; 32]),
                                timestamp: Utc::now(),
                            }
                        };

                        Self::apply_and_update_parents(new_tree, mut_fn, path)
                    } else {
                        // If so, call recursively
                        Self::apply_and_update_parents(
                            new_content.remove(elem_pos.try_into().unwrap()),
                            mut_fn,
                            path,
                        )
                    };

                    // Check if there is an error
                    if let Ok(mut tree) = result {
                        // If not, apply function, update hash and push

                        if !has_next {
                            tree = mut_fn(tree);
                        }

                        // Check if the fonction didn't return Empty
                        match tree {
                            Void => {}
                            File { .. } => new_content.push(tree),
                            Directory { ref content, .. } => {
                                if !content.is_empty() {
                                    new_content.push(tree);
                                }
                            }
                        }

                        Ok(Directory {
                            name,
                            content: new_content,
                            hash,
                        })
                    } else {
                        result
                    }
                }

                File { name, .. } => Err(NotADirectory(name)),

                Void => Err(NotADirectory("Void".to_string())),
            };

            match rtn {
                Ok(mut result) => {
                    result.update_hash();
                    Ok(result)
                }
                Err(e) => {
                    Err(e)
                }
            }
        } else {
            // If so, return parent
            Ok(parent)
        }
    }

    /// Non recursively update the hash of the directory (computed only with direct children).
    fn update_hash(&mut self) {
        if let Directory { hash, content, .. } = self {
            // Delete empty items
            content.retain(|e| !matches!(e, Void));

            if content.is_empty() {
                *hash = Hash::from_bytes([0; 32]);
            } else {
                *hash = Self::compute_content_hash(content);
            }
        }
    }

    /// Get node hash.
    pub fn hash(&self) -> Hash {
        match self {
            Void => Hash::from_bytes([0; 32]),
            File { hash, .. } => *hash,
            Directory { hash, .. } => *hash,
        }
    }

    /// Non-recursively compute the hash of content (computed only with direct children)
    fn compute_content_hash(content: &Vec<HashTree>) -> Hash {
        let mut data: Vec<u8> = Vec::new();
        for item in content {
            match item {
                Void => {}
                File { name, hash, .. } | Directory { name, hash, .. } => {
                    data.extend(name.as_bytes());
                    data.extend(hash.as_bytes());
                }
            }
        }
        blake3::hash(data.as_slice())
    }

    fn flatten(&self) -> Vec<String> {
        match self {
            Void | File { .. } => Vec::new(),
            Directory { content, .. } => {
                let mut rtn = Vec::new();

                for tree in content {
                    rtn.extend(tree.flatten_recursive("".to_string()));
                }

                rtn
            }
        }
    }

    fn flatten_recursive(&self, prefix: String) -> Vec<String> {
        match self {
            Void => Vec::new(),
            File { name, .. } => vec![prefix + name.as_str()],
            Directory { name, content, .. } => {
                let new_prefix = prefix + name.as_str() + "/";
                let mut rtn = Vec::new();

                for tree in content {
                    rtn.extend(tree.flatten_recursive(new_prefix.clone()));
                }

                rtn
            }
        }
    }

    /// Generate [`HashTree`] from disk.
    pub fn from_disk(dir: &SharedDirectory) -> Result<Self, std::io::Error> {
        let mut rtn = Directory {
            name: dir.path.file_name().unwrap().to_string_lossy().to_string(),
            content: vec![],
            hash: Hash::from_bytes([0; 32]),
        };

        let files_iter = WalkDir::new(dir.path())
            .follow_links(false)
            .same_file_system(true)
            .into_iter();

        let mut hasher = blake3::Hasher::new();

        for file_result in files_iter {
            match file_result {
                Ok(file) => {
                    // Ignore if it is a directory
                    if let Ok(metadata) = file.path().metadata() {
                        if !metadata.is_file() {
                            continue;
                        }
                    } else {
                        warn!("Cannot retrieve metatdata of '{}'", file.path().display());
                        continue
                    }

                    hasher
                        .update_mmap(file.path())
                        .inspect_err(|_| error!("Invalid path: {}", file.path().display()))?;

                    let relative_path = crate::engine::fs::relative(dir, file.path());

                    if relative_path.is_none() {
                        continue;
                    }

                    match rtn.apply(&Mutation::Modify {
                        file_path: relative_path.unwrap().to_string_lossy().to_string(),
                        file_hash: hasher.finalize(),
                        timestamp: Utc::now(),
                    }) {
                        Ok(new_rtn) => {
                            rtn = new_rtn;
                        }
                        Err(e) => {
                            warn!("Unable to add '{}' into hash tree ({e})", file.path().display())
                        }
                    }
                }
                Err(e) => {
                    warn!("Unable to scan file: {e}")
                }
            }
        }

        rtn.update_hash();
        Ok(rtn)
    }

    /// Generate all [`Mutation`] detected from current [`HashTree`].
    pub fn mutations_from_disk(&self, dir: &SharedDirectory) -> Vec<Mutation> {
        let mut current_files = self.flatten();
        let mut rtn = Vec::new();

        let files_iter = WalkDir::new(dir.path())
            .follow_links(false)
            .same_file_system(true)
            .into_iter();

        let mut hasher = blake3::Hasher::new();

        for file_result in files_iter {
            match file_result {
                Ok(file) => {
                    if let Ok(metadata) = file.path().metadata() {
                        if metadata.is_dir() {
                            continue;
                        }

                        if let Some(relative_path) = crate::engine::fs::relative(dir, file.path()) {
                            let relative_path_string = relative_path.to_string_lossy().to_string();

                            // Remove the file from the current file
                            current_files.retain(|e| e != &relative_path_string);

                            // Check if the file is in the hash tree
                            if let Some(File { timestamp, .. }) = self.get(&relative_path_string) {
                                // If so, check the timestamp
                                if let Ok(modified) = metadata.modified() {
                                    let new_timestamp: DateTime<Utc> = DateTime::from(modified);

                                    // Check if the saved timestamp is more recent
                                    if *timestamp >= new_timestamp {
                                        // If so, continue
                                        continue;
                                    }
                                }
                            }

                            // If we can't verify the file, re-hash it
                            if let Err(e) = hasher.update_mmap(file.path()) {
                                warn!("Unable to hash: '{}' ({})", file.path().display(), e);
                            }

                            // Push the mutation
                            rtn.push(Mutation::Modify {
                                file_path: relative_path_string.clone(),
                                file_hash: hasher.finalize(),
                                timestamp: Utc::now(),
                            })
                        }
                    } else {
                        warn!("Cannot retrieve metatdata of '{}'", file.path().display());
                        continue;
                    }
                }
                Err(e) => {
                    warn!("Unable to scan file: {e}")
                }
            }
        }

        // Add all file that were removed
        for missing in current_files {
            rtn.push(Mutation::Remove {
                file_path: missing,
                timestamp: Utc::now(),
            })
        }

        rtn
    }

    /// Return true if empty ([`Void`] or empty [`Directory`]).
    pub fn is_empty(&self) -> bool {
        match self {
            Void => true,
            Directory { content, .. } => content.is_empty(),
            File { .. } => false,
        }
    }
}

impl PartialEq<HashTree> for HashTree {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Void, Void) => true,
            (File { hash: h1, .. }, File { hash: h2, .. }) => h1 == h2,
            (Directory { hash: h1, .. }, Directory { hash: h2, .. }) => h1 == h2,
            _ => false,
        }
    }
}

impl Display for HashTree {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Void => Ok(()),
            File {
                name,
                hash,
                timestamp,
                ..
            } => {
                writeln!(f, "{}", *hash)?;
                writeln!(f, "Timestamp: {}", *timestamp)?;
                writeln!(f, "Name: {}", name)?;

                Ok(())
            }
            Directory {
                name,
                hash,
                content,
                ..
            } => {
                writeln!(f, "{}", *hash)?;
                writeln!(f, "Directory name: {}", name)?;

                if content.is_empty() {
                    return Ok(());
                } else {
                    writeln!(f, "│")?;
                }

                let last = content.len() - 1;

                for (i, tree) in content.iter().enumerate() {
                    let display = tree.to_string();
                    let mut iter = display.split('\n');

                    let mut prefix = "│";

                    if i == last {
                        writeln!(f, "└─{}", iter.next().unwrap())?;
                        prefix = " "
                    } else {
                        writeln!(f, "├─{}", iter.next().unwrap())?;
                    }

                    for line in iter {
                        writeln!(f, "{prefix} {}", line)?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Error, Debug)]
pub enum StateError {
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Not a directory: {0:?}")]
    NotADirectory(String),

    #[error("Hashing error: {0}")]
    Hashing(std::io::Error),

    #[error("Root node has no tree: {0}")]
    InvalidRoot(Hash),
}
