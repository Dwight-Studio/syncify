use crate::engine::state::{HashTree, Mutation};
use crate::SharedDirectory;
use chrono::Utc;
use iroh_gossip::net::GossipSender;
use log::{error, info};
use notify::event::{ModifyKind, RemoveKind, RenameMode};
use notify::EventKind::{Modify, Remove};
use std::path::Path;

pub struct FileSystemManager {
    topic: GossipSender,
    dir: SharedDirectory,
    mutations_buffer: Vec<Mutation>,
    last_tree: HashTree,
}

impl FileSystemManager {
    pub(crate) fn new(topic: GossipSender, dir: SharedDirectory, last_tree: HashTree) -> Self {
        Self {
            topic,
            dir,
            mutations_buffer: Vec::new(),
            last_tree,
        }
    }

    pub(crate) async fn handle_events(&mut self, fs_event: notify::Event) {
        let mut hasher = blake3::Hasher::new();

        match fs_event.kind {
            Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let (Some(path_from), Some(path_to)) = (
                    relative(&self.dir, &fs_event.paths[0]),
                    relative(&self.dir, &fs_event.paths[1]),
                ) {
                    info!(
                        "File {:?} moved to {:?} in {}",
                        path_from,
                        path_to,
                        self.dir.uuid()
                    );

                    let mutation = Mutation::Move {
                        from: path_from.to_string_lossy().to_string(),
                        to: path_to.to_string_lossy().to_string(),
                        timestamp: Utc::now().timestamp(),
                    };

                    self.mutations_buffer.push(mutation);
                }
            }
            Modify(ModifyKind::Data(_)) | Modify(ModifyKind::Name(RenameMode::To)) => {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(&self.dir, &abs_path) {
                        info!("File {:?} modified in {}", path, self.dir.uuid());
                        let file_hash = match hasher.update_mmap(&abs_path) {
                            Ok(hash) => *hash.finalize().as_bytes(),
                            Err(e) => {
                                error!("Cannot compute hash: {}", path.display());
                                continue;
                            }
                        };

                        let mutation = Mutation::Modify {
                            file_path: path.to_string_lossy().to_string(),
                            file_hash,
                            timestamp: Utc::now().timestamp(),
                        };

                        self.mutations_buffer.push(mutation);
                    }
                }
            }
            Remove(RemoveKind::File) | Modify(ModifyKind::Name(RenameMode::From)) => {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(&self.dir, &abs_path) {
                        info!("File {:?} removed in {}", path, self.dir.uuid());

                        let mutation = Mutation::Remove {
                            file_path: path.to_string_lossy().to_string(),
                            timestamp: Utc::now().timestamp(),
                        };

                        self.mutations_buffer.push(mutation);
                    }
                }
            }
            _ => return,
        }

        self.clean_mutation_buffer()
    }

    fn clean_mutation_buffer(&mut self) {
        let mut working_buffer = Vec::new();

        for n_mut in &self.mutations_buffer {
            
            // Fuse Mod+Mod and Rem+Mod
            working_buffer.retain(|o_mut| match (n_mut, o_mut) {
                (
                    Mutation::Modify {
                        file_path: n_path,
                        timestamp: n_time,
                        ..
                    },
                    Mutation::Modify {
                        file_path: o_path,
                        timestamp: o_time,
                        ..
                    },
                ) | (
                    Mutation::Remove {
                        file_path: n_path,
                        timestamp: n_time,
                        ..
                    },
                    Mutation::Modify {
                        file_path: o_path,
                        timestamp: o_time,
                        ..
                    },
                ) => {
                    // Retain if the path is different, or it's more recent
                    n_path != o_path || n_time < o_time
                }
                _ => true,
            });

            // Drop remove if about a file that doesn't exist
            if let Mutation::Remove { file_path, .. } = n_mut {
                if self.last_tree.get(file_path).is_none() {
                    continue;
                }
            }
            
            // TODO: Add move
            
            working_buffer.push(n_mut.clone());
        }
        
        // Copy the new buffer
        self.mutations_buffer = working_buffer;

        for mutation in &self.mutations_buffer {
            info!("{mutation}")
        }
    }
}

pub fn relative<'a>(dir: &SharedDirectory, file: &'a Path) -> Option<&'a Path> {
    match file.strip_prefix(dir.path.as_path()) {
        Ok(path) => Some(path),
        Err(_) => {
            error!("Cannot get relative path: {:?} in {:?}", file, dir.path);
            None
        }
    }
}
