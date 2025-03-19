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
use crate::engine::state::{HashTree, Mutation};
use crate::SharedDirectory;
use chrono::{DateTime, Utc};
use iroh_gossip::net::GossipSender;
use log::{error, info};
use notify::event::{ModifyKind, RemoveKind, RenameMode};
use notify::EventKind::{Modify, Remove};
use std::path::Path;

pub struct FileSystemManager {
    topic: GossipSender,
    dir: SharedDirectory,
    pub(super) remote_buffer: Vec<Mutation>,
    pub(super) local_buffer: Vec<Mutation>,
    last_tree: HashTree,
}

impl FileSystemManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory) -> Self {
        let inner = dir.inner.read().await;
        
        let mut local_buffer = if dir.is_read_only() {
            Vec::new()
        } else {
            inner.state.hash_tree().mutations_from_disk(&dir)
        };
        
        let last_tree = inner.state.hash_tree().clone();
        
        Self::clean_mutation_buffer(&mut local_buffer, Some(&last_tree));
        
        Self {
            topic,
            dir: dir.clone(),
            remote_buffer: Vec::new(),
            local_buffer,
            last_tree,
        }
    }

    pub(crate) async fn handle_events(&mut self, fs_event: notify::Event, timestamp: DateTime<Utc>) {
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
                        timestamp,
                    };

                    self.local_buffer.push(mutation);
                }
            }
            Modify(ModifyKind::Data(_)) | Modify(ModifyKind::Name(RenameMode::To)) => {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(&self.dir, &abs_path) {
                        info!("File '{:?}' modified in {}", path.display(), self.dir.uuid());
                        let file_hash = match hasher.update_mmap(&abs_path) {
                            Ok(hash) => *hash.finalize().as_bytes(),
                            Err(e) => {
                                error!("Cannot compute hash: {}", path.display());
                                continue;
                            }
                        };

                        let mutation = Mutation::Modify {
                            file_path: path.to_string_lossy().to_string(),
                            file_hash: Hash::from(file_hash),
                            timestamp,
                        };

                        self.local_buffer.push(mutation);
                    }
                }
            }
            Remove(RemoveKind::File) | Modify(ModifyKind::Name(RenameMode::From)) => {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(&self.dir, &abs_path) {
                        info!("File '{:?}' removed in {}", path.display(), self.dir.uuid());

                        let mutation = Mutation::Remove {
                            file_path: path.to_string_lossy().to_string(),
                            timestamp,
                        };

                        self.local_buffer.push(mutation);
                    }
                }
            }
            _ => return,
        }

        Self::clean_mutation_buffer(&mut self.local_buffer, Some(&self.last_tree));
    }

    fn clean_mutation_buffer(buffer: &mut Vec<Mutation>, tree: Option<&HashTree>) {
        let mut working_buffer = Vec::new();

        for n_mut in &*buffer {

            // Fuse Mod then Mod, Mod then Rem and Rem then Mod
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
                ) | (
                    Mutation::Modify {
                        file_path: n_path,
                        timestamp: n_time,
                        ..
                    },
                    Mutation::Remove {
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
            if let Some(last_tree) = tree {
                if let Mutation::Remove { file_path, .. } = n_mut {
                    if last_tree.get(file_path).is_none() {
                        continue;
                    }
                }
            }

            // TODO: Add move

            working_buffer.push(n_mut.clone());
        }

        // Copy the new buffer
        buffer.clear();
        buffer.extend(working_buffer);

        for mutation in buffer {
            info!("{mutation}")
        }
    }

    pub(crate) async fn apply_local_mutations(&mut self) {
        let write_key = match &self.dir.sign_key {
            Some(key) => key,
            None => return,
        };
        
        let mut inner = self.dir.inner.write().await;  
        
        info!("Applying mutations for {}", self.dir.uuid());
        
        for mutation in &self.local_buffer {
            match inner.state.mutate(mutation.clone(), write_key) {
                Ok(_) => {
                    info!("Applied: {mutation}");
                }
                Err(e) => {
                    error!("Cannot apply mutation: {mutation} ({e}");
                }
            }
        }
        
        self.local_buffer.clear();
        
        self.last_tree = inner.state.hash_tree().clone();
        info!("\n{}", self.last_tree)
    }
    
    pub(crate) async fn apply_remote_mutations(&mut self, mutations: Vec<Mutation>) {
        for mutation in mutations {
            self.remote_buffer.insert(0, mutation);
        }
    }
}

pub fn relative<'a>(dir: &SharedDirectory, file: &'a Path) -> Option<&'a Path> {
    match file.strip_prefix(dir.path.as_path()) {
        Ok(path) => Some(path),
        Err(_) => {
            error!("Cannot get relative path: '{:?}' in '{:?}'", file.display(), dir.path.display());
            None
        }
    }
}
