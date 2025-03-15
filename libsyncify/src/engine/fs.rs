use crate::engine::state::Mutation;
use crate::SharedDirectory;
use iroh_gossip::net::GossipSender;
use log::{error, info};
use notify::event::{ModifyKind, RemoveKind};
use notify::EventKind::{Modify, Remove};
use std::path::{Path, PathBuf};

pub async fn handle_events(dir: &SharedDirectory, topic: &GossipSender, fs_event: notify::Event) {
    //info!("Dir {}: {:?}", dir.uuid(), fs_event.kind);
    match fs_event.kind {
        Modify(kind) => match kind {
            ModifyKind::Data(_) => {
                
            }
            ModifyKind::Name(_) => {
                
            }
            _ => {}
        },
        Remove(kind) => {
            if kind == RemoveKind::File {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(dir, &abs_path) {
                        info!("File {:?} removed in {}", path, dir.uuid());

                        match dir
                            .state.write().await
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
        },
    }
}