use crate::engine::state::Mutation;
use crate::SharedDirectory;
use iroh_gossip::net::GossipSender;
use log::{error, info};
use notify::event::{ModifyKind, RemoveKind, RenameMode};
use notify::EventKind::{Modify, Remove};
use std::hash::Hasher;
use std::path::{Path, PathBuf};

pub async fn handle_events(dir: &SharedDirectory, topic: &GossipSender, fs_event: notify::Event) {
    let mut hasher = blake3::Hasher::new();

    match fs_event.kind {
        Modify(ModifyKind::Data(_)) => {
            for abs_path in fs_event.paths {
                if let Some(path) = relative(dir, &abs_path) {
                    info!("File {:?} modified in {}", path, dir.uuid());

                    let file_hash = match hasher.update_mmap(&abs_path) {
                        Ok(hash) => hash.finalize().as_bytes().clone(),
                        Err(e) => {
                            error!("Cannot compute hash: {}", path.display());
                            continue;
                        }
                    };

                    match dir
                        .state
                        .write()
                        .await
                        .mutate(Mutation::Modify {
                            file_path: path.to_string_lossy().to_string(),
                            file_hash,
                        })
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to modify file {:?} in {} ({e})", path, dir.uuid());
                        }
                    }
                }
            }
        },
        // TODO: Check if the file exists in db
        Remove(RemoveKind::File) | Modify(ModifyKind::Name(RenameMode::From)) => {
            for abs_path in fs_event.paths {
                if let Some(path) = relative(dir, &abs_path) {
                    info!("File {:?} removed in {}", path, dir.uuid());

                    match dir
                        .state
                        .write()
                        .await
                        .mutate(Mutation::Remove {
                            file_path: path.to_string_lossy().to_string(),
                        })
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Failed to remove file {:?} in {} ({e})", path, dir.uuid());
                        }
                    }
                }
            }
        }
        _ => (),
    }
}

fn relative<'a>(dir: &SharedDirectory, file: &'a PathBuf) -> Option<&'a Path> {
    match file.strip_prefix(dir.path.parent()?) {
        Ok(path) => Some(path),
        Err(_) => {
            error!("Cannot get relative path: {:?} in {:?}", file, dir.path);
            None
        }
    }
}
