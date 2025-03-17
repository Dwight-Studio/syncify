use crate::engine::state::Mutation;
use crate::SharedDirectory;
use chrono::Utc;
use iroh_gossip::net::GossipSender;
use log::{error, info};
use notify::event::{ModifyKind, RemoveKind, RenameMode};
use notify::EventKind::{Modify, Remove};
use std::path::Path;

pub async fn handle_events(dir: &SharedDirectory, topic: &GossipSender, fs_event: notify::Event, mutations_buffer: &mut Vec<Mutation>) {
    let mut hasher = blake3::Hasher::new();

    match fs_event.kind {
        Modify(ModifyKind::Name(RenameMode::Both)) => {
            if let (Some(path_from), Some(path_to)) = (relative(dir, &fs_event.paths[0]), relative(dir, &fs_event.paths[1])) {
                info!("File {:?} moved to {:?} in {}", path_from, path_to, dir.uuid());

                let mutation = Mutation::Move {
                    from: path_from.to_string_lossy().to_string(),
                    to: path_to.to_string_lossy().to_string(),
                    timestamp: Utc::now().timestamp(),
                };

                mutations_buffer.push(mutation);
            }
        },
        Modify(ModifyKind::Data(_)) | Modify(ModifyKind::Name(RenameMode::To)) => {
            for abs_path in fs_event.paths {
                if let Some(path) = relative(dir, &abs_path) {
                    info!("File {:?} modified in {}", path, dir.uuid());
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
                        timestamp: Utc::now().timestamp()
                    };

                    mutations_buffer.push(mutation);
                }
            }
        }
        Remove(RemoveKind::File) | Modify(ModifyKind::Name(RenameMode::From)) => {
            for abs_path in fs_event.paths {
                if let Some(path) = relative(dir, &abs_path) {
                    info!("File {:?} removed in {}", path, dir.uuid());

                    let mutation = Mutation::Remove {
                        file_path: path.to_string_lossy().to_string(),
                        timestamp: Utc::now().timestamp(),
                    };

                    mutations_buffer.push(mutation);
                }
            }
        }
        _ => (),
    }

    //deduplicate_buffer(mutations_buffer)
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

async fn deduplicate_buffer(mutations_buffer: &mut Vec<Mutation>, dir: &SharedDirectory) {
    //let tree = dir.inner.read().await.state.hash_tree();

    let mut modified= Vec::new();
    let mut removed = Vec::new();
    let mut moved = Vec::new();

    // Classify each mutation
    for mutation in mutations_buffer {
        match mutation {
            Mutation::Modify { .. } => modified.push(mutation.clone()),
            Mutation::Move { .. } => moved.push(mutation.clone()),
            Mutation::Remove { .. } => removed.push(mutation.clone()),
            _ => {}
        }
    }

    // TODO: Drop the remove/modify with the move

    // Drop every modified if removed after modification
    for rm in &removed {
        if let Mutation::Remove { file_path: r_path, timestamp: r_time, .. } = rm {
            modified.retain(|mm| {
                if let Mutation::Modify { file_path: m_path, timestamp: m_time, .. } = mm {
                    // If the remove is after the modification, and has the same path
                    if *r_time > *m_time && r_path == m_path {
                        return false
                    }
                }

                // By default, keep
                true
            })
        }
    }
}
