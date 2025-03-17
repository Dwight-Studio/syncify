use crate::engine::state::Mutation;
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
}

impl FileSystemManager {
    pub(crate) fn new(topic: GossipSender, dir: SharedDirectory) -> Self {
        Self {
            topic,
            dir,
            mutations_buffer: Vec::new(),
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
            _ => (),
        }

        //deduplicate_buffer(mutations_buffer)
    }

    async fn deduplicate_buffer(&mut self, dir: &SharedDirectory) {
        //let tree = dir.inner.read().await.state.hash_tree();

        let mut modified = Vec::new();
        let mut removed = Vec::new();
        let mut moved = Vec::new();

        // Classify each mutation
        for mutation in &self.mutations_buffer {
            match mutation {
                Mutation::Modify { .. } => modified.push(mutation.clone()),
                Mutation::Move { .. } => moved.push(mutation.clone()),
                Mutation::Remove { .. } => removed.push(mutation.clone()),
                _ => {}
            }
        }

        // TODO: Drop the remove for a file that doesn't exist

        // TODO: Drop the remove/modify if it is in fact a move

        // Drop every modified if removed after modification
        for rm in &removed {
            if let Mutation::Remove {
                file_path: r_path,
                timestamp: r_time,
                ..
            } = rm
            {
                modified.retain(|mm| {
                    if let Mutation::Modify {
                        file_path: m_path,
                        timestamp: m_time,
                        ..
                    } = mm
                    {
                        // If the remove is after the modification, and has the same path
                        if *r_time > *m_time && r_path == m_path {
                            return false;
                        }
                    }

                    // By default, keep
                    true
                })
            }
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
