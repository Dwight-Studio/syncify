use crate::engine::state::HashTree::{Directory, Empty, File};
use crate::engine::state::StateError::NotADirectory;
use blake3::Hash;
use std::cell::RefCell;
use std::cmp::PartialEq;
use std::iter::Peekable;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub struct State {
    head: Rc<RefCell<Delta>>,
}

impl State {
    pub fn new() -> Self {
        let duration_since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self {
            head: Rc::new(RefCell::new(Delta {
                parent: None,
                hash: blake3::hash(&duration_since_epoch.as_millis().to_be_bytes()),
                hash_tree_cache: Some(HashTree::Empty),
                action: Mutation::Init,
            })),
        }
    }

    /// Get parent state.
    pub fn parent(&self) -> Option<Self> {
        if self.head.borrow().parent.is_some() {
            Some(Self {
                head: self.head.borrow().parent.clone().unwrap(),
            })
        } else {
            None
        }
    }

    /// Get file hash tree.
    pub fn hash_tree(&mut self) -> Result<HashTree, StateError> {
        self.head.borrow_mut().compute_hash_tree()?;
        Ok(self.head.borrow().hash_tree_cache.clone().unwrap())
    }
}

#[derive(Clone)]
struct Delta {
    parent: Option<Rc<RefCell<Delta>>>,
    hash: Hash,
    hash_tree_cache: Option<HashTree>,
    action: Mutation,
}

impl Delta {
    /// Recursively compute hash tree.
    fn compute_hash_tree(&mut self) -> Result<(), StateError> {
        if self.hash_tree_cache.is_some() {
            Ok(())
        } else {
            if let Some(parent) = &self.parent {
                parent.borrow_mut().compute_hash_tree()?;
                match &parent.borrow().hash_tree_cache {
                    None => {}
                    Some(cache) => {
                        self.hash_tree_cache = Some(cache.apply(&self.action)?);
                    }
                }
            } else {
                self.hash_tree_cache = Some(Empty);
            }
            Ok(())
        }
    }
}

#[derive(Clone)]
enum Mutation {
    Init,
    Merge { parent: Rc<Delta> },
    Modify { file_path: String, file_hash: Hash },
    Move { from: String, to: String },
    Remove { file_path: String },
}

#[derive(Clone)]
pub enum HashTree {
    Empty,
    File {
        name: String,
        hash: Hash,
    },
    Directory {
        name: String,
        content: Vec<HashTree>,
        hash: Hash,
    },
}

impl HashTree {
    /// Generate a vec of references of the file/directory at given path, and its parent in reverse
    /// hierarchical order.
    fn goto<'a>(
        &self,
        mut file_path_iter: impl Iterator<Item = &'a str> + Clone,
    ) -> Option<Vec<&HashTree>> {
        let dir_name = file_path_iter.next()?;
        match self {
            HashTree::Empty => None,
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
                        hash: file_hash.clone(),
                    }
                },
                file_path.split("/").peekable(),
            ),
            Mutation::Move { from, to } => {
                let extracted = self.goto(from.split("/")).unwrap()[0];
                let tree = Self::apply_and_update_parents(
                    self.clone(),
                    &mut |_: HashTree| -> HashTree { HashTree::Empty },
                    from.split("/").peekable(),
                )?;
                Self::apply_and_update_parents(
                    tree,
                    &mut |_: HashTree| -> HashTree { extracted.clone() },
                    from.split("/").peekable(),
                )
            }
            Mutation::Remove { file_path } => Self::apply_and_update_parents(
                self.clone(),
                &mut |_: HashTree| -> HashTree { HashTree::Empty },
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
                let mut pos = 0;
                let mut elem_pos: i64 = -1;
                for tree in content {
                    match &tree {
                        HashTree::Empty => {}
                        File { name, .. } | Directory { name, .. } => {
                            if name == elem {
                                elem_pos = pos;
                            }
                        }
                    }
                    new_content.push(tree);
                    pos += 1;
                }

                let result;

                // Check if the parent contains the next elem
                if elem_pos == -1 {
                    // If not, let the function construct the object
                    let new_tree;
                    if has_next {
                        new_tree = Directory {
                            name: elem.to_string(),
                            content: Vec::new(),
                            hash: Hash::from_bytes([0; 32]),
                        };
                    } else {
                        new_tree = File {
                            name: elem.to_string(),
                            hash: Hash::from_bytes([0; 32]),
                        };
                    }

                    result = Self::apply_and_update_parents(new_tree, mut_fn, path);
                } else {
                    // If so, call recursively
                    result = Self::apply_and_update_parents(
                        new_content.remove(pos.try_into().unwrap()),
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
                    if tree != HashTree::Empty {
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
                Err(NotADirectory(elem.to_string()))
            }
        } else {
            // If so, return parent
            Ok(parent)
        }
    }

    /// Non recursively update the hash of the directory (computed only with direct children).
    fn update_hash(&mut self) {
        match self {
            Directory { hash, content, .. } => {
                // Delete empty items
                content.retain(|e| !matches!(e, Empty));

                if content.len() == 0 {
                    *hash = Hash::from_bytes([0; 32]);
                } else {
                    *hash = Self::compute_content_hash(content);
                }
            }
            _ => {}
        }
    }

    /// Non-recursively compute the hash of content (computed only with direct children)
    fn compute_content_hash(content: &Vec<HashTree>) -> Hash {
        let mut data: Vec<u8> = Vec::new();
        for item in content {
            match item {
                Empty => {}
                File { name, hash, .. } | Directory { name, hash, .. } => {
                    data.extend_from_slice(name.as_bytes());
                    data.extend_from_slice(hash.as_bytes());
                }
            }
        }
        blake3::hash(data.as_slice())
    }
}

impl PartialEq<HashTree> for HashTree {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Empty, Empty) => true,
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

    #[error("Not a directory: {0}")]
    NotADirectory(String),
}
