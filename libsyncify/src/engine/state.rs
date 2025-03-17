use crate::engine::state::HashTree::{Directory, File, Void};
use crate::engine::state::StateError::NotADirectory;
use crate::SharedDirectory;
use blake3::Hash;
use chrono::{DateTime, Utc};
use log::{error, info, warn};
use rkyv::{Archive, Deserialize, Serialize};
use std::cmp::PartialEq;
use std::fmt::{Display, Formatter};
use std::fs;
use std::iter::Peekable;
use std::sync::{Arc, RwLock, RwLockReadGuard};
use thiserror::Error;
use walkdir::WalkDir;

pub const MAX_LOADED_DELTAS: u32 = 16384;
pub const MAX_UNFLUSHED_DELTAS: u32 = MAX_LOADED_DELTAS * 32;

/// Tree containing the synchronisation information for a [`SharedDirectory`].
#[derive(Clone, Debug)]
pub struct State {
    pub(crate) head: Arc<RwLock<Delta>>,
    /// The timestamp is dated from last time it was "seen" out of the cache
    /// i.e. last time it was saved, loaded or synced.
    pub(crate) timestamp: DateTime<Utc>,
}

// TODO: Add optimization
//  -> Compute when last modified date is > than the last save date (for fastforward sync)
impl State {
    pub fn new(directory_name: String) -> Self {
        let timestamp = Utc::now();
        Self {
            head: Arc::new(RwLock::new(Delta {
                parent: None,
                hash: blake3::hash(&timestamp.timestamp().to_be_bytes()),
                hash_tree_cache: Some(Directory {
                    name: directory_name,
                    content: vec![],
                    hash: [0; 32],
                }),
                action: Mutation::Init { timestamp: Utc::now().timestamp() },
                timestamp,
            })),
            timestamp,
        }
    }

    /// Get parent state.
    pub fn parent(&self) -> Option<Self> {
        let head = self.head.read().unwrap();
        if head.parent.is_some() {
            let parent = head.parent.clone().unwrap();

            Some(Self {
                head: parent.clone(),
                timestamp: parent.read().unwrap().timestamp,
            })
        } else {
            None
        }
    }

    pub fn head(&self) -> RwLockReadGuard<'_, Delta> {
        self.head.read().unwrap()
    }

    /// Get file hash tree.
    pub fn hash_tree(&mut self) -> Result<HashTree, StateError> {
        let mut head = self.head.write().unwrap();
        head.compute_hash_tree()?;
        Ok(head.hash_tree_cache.clone().unwrap())
    }
    
    pub fn mutate(&mut self, mutation: Mutation) -> Result<(), StateError> {
        let mut delta = Delta {
            parent: Some(self.head.clone()),
            hash: Hash::from_bytes([0; 32]),
            hash_tree_cache: None,
            action: mutation,
            timestamp: Utc::now(),
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
    pub fn prune(&mut self) -> bool {
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

    pub fn iter(&self) -> StateIterator {
        StateIterator {
            head_ref: Some(self.head.clone()),
        }
    }
}

impl Display for State {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        writeln!(f, "State")?;
        writeln!(f, "Timestamp: {}", self.timestamp)?;
        writeln!(f)?;
        writeln!(f, "■ Current")?;

        for head_ref in self.iter() {
            let head = head_ref.read().unwrap();

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
    head_ref: Option<Arc<RwLock<Delta>>>,
}

impl Iterator for StateIterator {
    type Item = Arc<RwLock<Delta>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.head_ref.clone() {
            Some(head_ref) => {
                let rtn = head_ref.clone();

                let head = head_ref.read().unwrap();
                self.head_ref = head.parent.clone();

                Some(rtn)
            }
            None => None,
        }
    }
}

/// Node describing a modification of a [`State`].
#[derive(Clone, Debug)]
pub struct Delta {
    pub(super) parent: Option<Arc<RwLock<Delta>>>,
    pub(super) hash: Hash,
    pub(super) hash_tree_cache: Option<HashTree>,
    pub(super) action: Mutation,
    pub(super) timestamp: DateTime<Utc>,
}

impl Delta {
    /// Recursively compute hash tree if the cache is empty.
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
                return Err(StateError::InvalidRoot(self.hash));
            }
            Ok(())
        }
    }

    pub fn hash(&self) -> Hash {
        self.hash
    }
}

/// Mutation action of a [`Delta`].
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum Mutation {
    Init {
        timestamp: i64
    },
    Merge {
        other_head: [u8; 32],
        timestamp: i64
    },
    Modify {
        file_path: String,
        file_hash: [u8; 32],
        timestamp: i64
    },
    Move {
        from: String,
        to: String,
        timestamp: i64
    },
    Remove {
        file_path: String,
        timestamp: i64
    },
}

impl Display for Mutation {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            Mutation::Init { .. } => write!(f, "Initialized directory"),
            Mutation::Merge { other_head, .. } => {
                write!(f, "Merged branch {}", Hash::from_bytes(*other_head))
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
        hash: [u8; 32],
        timestamp: i64,
    },
    Directory {
        name: String,
        #[rkyv(omit_bounds)]
        content: Vec<HashTree>,
        hash: [u8; 32],
    }
}

impl HashTree {
    /// Get a reference to the hash tree at the path.
    pub fn get(&self, file_path: &str) -> Option<&HashTree> {
        self.get_recursive(&mut file_path.split("/"))
    }

    fn get_recursive<'a, A>(&self, file_path_iter: &mut A) -> Option<&HashTree>
    where
        A: Iterator<Item = &'a str> + Clone,
    {
        let path = file_path_iter.next()?;
        
        match &self {
            Void => None,
            File { name, .. } => {
                if name == path {
                    Some(self)
                } else {
                    None
                }
            }
            Directory { name, content, .. } => {
                if name == path {
                    Some(self)
                } else {
                    for tree in content.iter() {
                        let mut iter = file_path_iter.clone();
                        
                        if let Some(tree) = tree.get_recursive(&mut iter) {
                            return Some(tree)
                        } 
                    }
                    
                    None
                }
            }
        }
    }

    /// Construct a mutated version of self.
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
                        timestamp: Utc::now().timestamp(),
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
            match parent {
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
                                hash: [0; 32],
                            }
                        } else {
                            File {
                                name: elem.to_string(),
                                hash: [0; 32],
                                timestamp: 0,
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
                }

                File { name, .. } => Err(NotADirectory(name)),

                Void => Err(NotADirectory("Void".to_string())),
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
    pub fn from_disk(dir: &SharedDirectory) -> Result<Self, std::io::Error> {
        let mut rtn = Directory {
            name: dir.path.file_name().unwrap().to_string_lossy().to_string(),
            content: vec![],
            hash: [0; 32],
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
                    if !fs::metadata(file.path()).is_ok_and(|e| e.is_file()) {
                        continue;
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
                        file_hash: *hasher.finalize().as_bytes(),
                        timestamp: Utc::now().timestamp(),
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
                writeln!(f, "{}", Hash::from_bytes(*hash))?;
                writeln!(
                    f,
                    "Timestamp: {}",
                    DateTime::<Utc>::from_timestamp(*timestamp, 0).unwrap()
                )?;
                writeln!(f, "Name: {}", name)?;

                Ok(())
            }
            Directory {
                name,
                hash,
                content,
                ..
            } => {
                writeln!(f, "{}", Hash::from_bytes(*hash))?;
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
            },
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
