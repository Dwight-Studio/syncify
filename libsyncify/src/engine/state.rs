use crate::engine::state::StateError::InvalidPath;
use std::cmp::PartialEq;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub struct State {
    head: Rc<Delta>,
}

impl State {
    pub fn new() -> Self {
        let duration_since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        Self {
            head: Rc::new(Delta {
                parent: None,
                hash: blake3::hash(&duration_since_epoch.as_millis().to_be_bytes()),
                file_tree: Some(HashTree::Empty),
                action: Mutation::Init,
            }),
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if self.head.parent.is_some() {
            Some(Self {
                head: self.head.parent.clone().unwrap(),
            })
        } else {
            None
        }
    }

    pub fn get_hash_tree(&mut self) -> &HashTree {
        if self.head.parent.is_some() {
            &self.head.file_tree.as_ref().unwrap()
        } else {
            while self.head.parent.is_none() {
                todo!()
            }

            &self.head.file_tree.as_ref().unwrap()
        }
    }
}

#[derive(Clone)]
struct Delta {
    parent: Option<Rc<Delta>>,
    hash: blake3::Hash,
    file_tree: Option<HashTree>,
    action: Mutation,
}

#[derive(Clone)]
enum Mutation {
    Init,
    Merge {
        parent: Rc<Delta>,
    },
    Modify {
        file_path: String,
        file_hash: blake3::Hash,
    },
    Move {
        from: String,
        to: String,
    },
    Remove {
        file_path: String,
    },
}

#[derive(Clone)]
pub enum HashTree {
    Empty,
    File {
        name: String,
        hash: blake3::Hash,
    },
    Directory {
        name: String,
        content: Vec<HashTree>,
        hash: blake3::Hash,
    },
}

impl HashTree {
    // TODO: Add traceback to root to be able to modify the hash of the parents
    fn goto<'a>(
        &self,
        mut file_path_iter: impl Iterator<Item = &'a str> + Clone,
    ) -> Option<Vec<&HashTree>> {
        let dir_name = file_path_iter.next()?;

        match self {
            HashTree::Empty => None,
            HashTree::File { .. } => Some(vec![self]),
            HashTree::Directory { name, content, .. } => {
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

    // TODO: Why not using recursive fn?
    fn apply(&self, mutation: Mutation) -> Result<HashTree, StateError> {
        match mutation {
            Mutation::Init => Ok(self.clone()),
            Mutation::Merge { parent } => todo!(),
            Mutation::Modify {
                file_path,
                file_hash,
            } => todo!(),
            Mutation::Move { from, to } => todo!(),
            Mutation::Remove { file_path } => {
                let path = self
                    .goto(file_path.split("/"))
                    .ok_or(InvalidPath(file_path.clone()))?;
                let elem = path[0];
                let parent;
                if let HashTree::Directory {
                    name,
                    mut content,
                    hash,
                } = path[1].clone()
                {
                    content.retain(|item| item != elem);
                    parent = HashTree::Directory {
                        name,
                        content,
                        hash,
                    };
                } else {
                    return Err(InvalidPath(file_path.clone()));
                }

                Ok(parent.propagate_update(path))
            }
        }
    }

    fn propagate_update(mut self, path: Vec<&HashTree>) -> Self {
        self.update_hash();

        for i in 2..path.len() {
            if let HashTree::Directory {
                name,
                mut content,
                hash,
            } = path[i].clone()
            {
                if let Some(p) = content.iter().position(|item| item == path[i - 1]) {
                    content[p] = self;
                    self = HashTree::Directory {
                        name,
                        content,
                        hash,
                    };
                } else {
                    panic!("Cannot propagate update")
                }
            }
        }

        self
    }

    fn update_hash(&mut self) -> Option<blake3::Hash> {
        match self {
            &mut HashTree::Directory {
                mut hash,
                ref mut content,
                ..
            } => {
                let mut data: Vec<u8> = Vec::new();
                for item in content {
                    match item {
                        HashTree::Empty => {}
                        HashTree::File { name, hash, .. }
                        | HashTree::Directory { name, hash, .. } => {
                            data.extend_from_slice(name.as_bytes());
                            data.extend_from_slice(hash.as_bytes());
                        }
                    }
                }
                hash = blake3::hash(data.as_slice());
                Some(hash)
            }
            _ => None,
        }
    }
}

impl PartialEq<HashTree> for HashTree {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (HashTree::Empty, HashTree::Empty) => true,
            (HashTree::File { hash: h1, .. }, HashTree::File { hash: h2, .. }) => h1 == h2,
            (HashTree::Directory { hash: h1, .. }, HashTree::Directory { hash: h2, .. }) => {
                h1 == h2
            }
            _ => false,
        }
    }
}

#[derive(Error, Debug)]
enum StateError {
    #[error("Invalid path: {0}")]
    InvalidPath(String),
}
