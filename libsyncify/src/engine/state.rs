use crate::engine::state::HashTree::{Directory, File, Void};
use crate::engine::state::StateError::NotADirectory;
use blake3::Hash;
use log::{error, info, warn};
use redb::{ReadableTable, Table, TypeName, Value};
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::fs;
use std::iter::Peekable;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use walkdir::WalkDir;

pub const MAX_LOADED_DELTAS: u32 = 16384;
pub const MAX_UNFLUSHED_DELTAS: u32 = MAX_LOADED_DELTAS * 32;

/// Tree containing the synchronisation information for a [`SharedDirectory`].
#[derive(Clone, Debug)]
pub struct State {
    pub(crate) head: Arc<RwLock<Delta>>,
}

// TODO: Add optimization
//  -> Fuse unshared changes if possible
//  -> Compute when last modified date is > than the last save date
impl State {
    pub fn new(directory_name: String) -> Self {
        let duration_since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self {
            head: Arc::new(RwLock::new(Delta {
                parent: None,
                hash: blake3::hash(&duration_since_epoch.as_millis().to_be_bytes()),
                hash_tree_cache: Some(Directory {
                    name: directory_name,
                    content: vec![],
                    hash: [0; 32],
                }),
                action: Mutation::Init,
            })),
        }
    }

    /// Get parent state.
    pub async fn parent(&self) -> Option<Self> {
        let head = self.head.read().unwrap();
        if head.parent.is_some() {
            Some(Self {
                head: head.parent.clone().unwrap(),
            })
        } else {
            None
        }
    }

    /// Get file hash tree.
    pub async fn hash_tree(&mut self) -> Result<HashTree, StateError> {
        let mut head = self.head.write().unwrap();
        head.compute_hash_tree()?;
        Ok(head.hash_tree_cache.clone().unwrap())
    }

    // TODO: Add optimisation: Detect if the same file is modified in the last delta and fuse
    pub async fn mutate(&mut self, mutation: Mutation) -> Result<(), StateError> {
        let mut delta = Delta {
            parent: Some(self.head.clone()),
            hash: Hash::from_bytes([0; 32]),
            hash_tree_cache: None,
            action: mutation,
        };

        // Compute hash tree
        delta.compute_hash_tree()?;
        delta.hash = {
            match &delta.hash_tree_cache {
                Some(tree) => {
                    let mut data = Vec::new();

                    // Parent
                    data.extend(self.head.read().unwrap().hash.as_bytes());

                    // Tree
                    data.extend(tree.get_hash().as_bytes());
                    blake3::hash(data.leak())
                }
                None => Hash::from_bytes([0; 32]),
            }
        };

        self.head = Arc::new(RwLock::new(delta));

        Ok(())
    }
    
    /// Prune tree if over the limit of loaded deltas.
    pub fn optimize(&mut self) -> bool {
        let mut head = self.head.clone();
        
        for i in 0..MAX_LOADED_DELTAS {
            if let Some(parent) = head.clone().read().unwrap().parent.clone() {
                head = parent;
            } else {
                return false;
            }
        }
        
        head.write().unwrap().parent = None;
        
        true
    }
}

/// Node describing a modification of a [`State`].
#[derive(Clone, Debug)]
pub struct Delta {
    pub(crate) parent: Option<Arc<RwLock<Delta>>>,
    pub(crate) hash: Hash,
    pub(crate) hash_tree_cache: Option<HashTree>,
    pub(crate) action: Mutation,
}

impl Delta {
    /// Recursively compute hash tree.
    fn compute_hash_tree(&mut self) -> Result<(), StateError> {
        if self.hash_tree_cache.is_some() {
            Ok(())
        } else {
            if let Some(parent_ref) = &self.parent {
                let mut parent = parent_ref.write().unwrap();
                parent.compute_hash_tree()?;
                match &parent.hash_tree_cache {
                    None => {}
                    Some(cache) => {
                        self.hash_tree_cache = Some(cache.apply(&self.action)?);
                    }
                }
            } else {
                self.hash_tree_cache = Some(Void);
            }
            Ok(())
        }
    }
}

/// Mutation action of a [`Delta`].
#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
pub enum Mutation {
    Init,
    Merge {
        other_head: [u8; 32],
    },
    Modify {
        file_path: String,
        file_hash: [u8; 32],
    },
    Move {
        from: String,
        to: String,
    },
    Remove {
        file_path: String,
    },
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
        hash: [u8; 32],
    },
    Directory {
        name: String,
        #[rkyv(omit_bounds)]
        content: Vec<HashTree>,
        hash: [u8; 32],
    },
}

impl HashTree {
    /// Generate a vec of references of the file/directory at given path, and its parent in reverse
    /// hierarchical order.
    pub fn goto<'a>(
        &self,
        mut file_path_iter: impl Iterator<Item = &'a str> + Clone,
    ) -> Option<Vec<&HashTree>> {
        let dir_name = file_path_iter.next()?;
        match self {
            Void => None,
            File { .. } => Some(vec![self]),
            Directory { name, content, .. } => {
                if name.eq(&dir_name) {
                    for tree in content {
                        let opt = tree.goto(file_path_iter.clone());
                        if opt.is_some() {
                            let mut parent = opt.unwrap();
                            parent.push(self);
                            return Some(parent);
                        }
                    }
                }
                None
            }
        }
    }

    /// Construct a mutated version of self.
    fn apply(&self, mutation: &Mutation) -> Result<HashTree, StateError> {
        match mutation {
            Mutation::Init => Ok(self.clone()),
            Mutation::Merge { .. } => Ok(self.clone()),
            Mutation::Modify {
                file_path,
                file_hash,
            } => Self::apply_and_update_parents(
                self.clone(),
                &mut |_: HashTree| -> HashTree {
                    File {
                        name: file_path.split("/").last().unwrap().to_string(),
                        hash: *file_hash,
                    }
                },
                file_path.split("/").peekable(),
            ),
            Mutation::Move { from, to } => {
                let extracted = self.goto(from.split("/")).unwrap()[0];
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
            Mutation::Remove { file_path } => Self::apply_and_update_parents(
                self.clone(),
                &mut |_: HashTree| -> HashTree { Void },
                file_path.split("/").peekable(),
            ),
        }
    }

    /// Apply a function on a file/folder provided with path, and update parents.
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
            if let Directory {
                name,
                content,
                hash,
            } = parent
            {
                // If so, construct new content
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

                let result;

                // Check if the parent contains the next elem
                if elem_pos == -1 {
                    // If not, let the function construct the object
                    let new_tree = if has_next {
                        Directory {
                            name: elem.to_string(),
                            content: Vec::new(),
                            hash: [0; 32],
                        }
                    } else {
                        File {
                            name: elem.to_string(),
                            hash: [0; 32],
                        }
                    };

                    result = Self::apply_and_update_parents(new_tree, mut_fn, path);
                } else {
                    // If so, call recursively
                    result = Self::apply_and_update_parents(
                        new_content.remove(elem_pos.try_into().unwrap()),
                        mut_fn,
                        path,
                    );
                }

                // Check if there is an error
                if let Ok(mut tree) = result {
                    // If not, apply function, update hash and push
                    // If the
                    if !has_next {
                        tree = mut_fn(tree);
                    }

                    // Check if the fonction didn't return Empty
                    if tree != Void {
                        // If not, update and push
                        tree.update_hash();
                        new_content.push(tree);
                    }

                    Ok(Directory {
                        name,
                        content: new_content,
                        hash,
                    })
                } else {
                    result
                }
            } else {
                // If not, there is an error (next is not None, and it reached a File)
                match parent {
                    File { name, .. } => Err(NotADirectory(name)),
                    Void => Err(NotADirectory("Void".to_string())),
                    _ => unreachable!(),
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
                *hash = [0; 32];
            } else {
                *hash = Self::compute_content_hash(content);
            }
        }
    }

    pub fn get_hash(&self) -> Hash {
        match self {
            Void => Hash::from_bytes([0; 32]),
            File { hash, .. } => Hash::from(*hash),
            Directory { hash, .. } => Hash::from(*hash),
        }
    }

    /// Non-recursively compute the hash of content (computed only with direct children)
    fn compute_content_hash(content: &Vec<HashTree>) -> [u8; 32] {
        let mut data: Vec<u8> = Vec::new();
        for item in content {
            match item {
                Void => {}
                File { name, hash, .. } | Directory { name, hash, .. } => {
                    data.extend(name.as_bytes());
                    data.extend(hash);
                }
            }
        }
        *blake3::hash(data.as_slice()).as_bytes()
    }

    /// Generate [`HashTree`] from disk.
    pub fn from_disk(path: &Path) -> Result<Self, std::io::Error> {
        let mut rtn = Directory {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            content: vec![],
            hash: [0; 32],
        };

        let files_iter = WalkDir::new(path)
            .follow_links(false)
            .same_file_system(true)
            .into_iter();

        let mut hasher = blake3::Hasher::new();

        for file_result in files_iter {
            match file_result {
                Ok(file) => {
                    // Ignore if it is a directory
                    if !fs::metadata(file.path()).is_ok_and(|e| e.is_file()) {
                        continue;
                    }
                    
                    hasher
                        .update_mmap(file.path())
                        .inspect_err(|_| error!("Invalid path: {}", file.path().display()))?;
                    match rtn.apply(&Mutation::Modify {
                        file_path: file
                            .path()
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string(),
                        file_hash: *hasher.finalize().as_bytes(),
                    }) {
                        Ok(new_rtn) => {
                            rtn = new_rtn;
                        }
                        Err(e) => {
                            warn!("Unable to add `{}` into hash tree", file.path().display())
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

#[derive(Error, Debug)]
pub enum StateError {
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Not a directory: {0:?}")]
    NotADirectory(String),

    #[error("Hashing error: {0}")]
    Hashing(std::io::Error),
}
