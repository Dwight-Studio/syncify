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
use crate::SharedDirectory;
use crate::engine::downloader::{DownloaderEvent, DownloaderHandle};
use crate::engine::job::{DownloadJob, JobState};
use crate::engine::state::{HashTree, Mutation};
use chrono::Utc;
use iroh_gossip::net::GossipSender;
use log::{debug, error};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;

pub struct FileSystemManager {
    topic: GossipSender,
    downloader: DownloaderHandle,
    dir: SharedDirectory,
}

impl FileSystemManager {
    pub async fn new(topic: GossipSender, downloader: DownloaderHandle, dir: SharedDirectory) -> Self {
        Self {
            topic,
            downloader,
            dir: dir.clone(),
        }
    }

    /// Poll the file for changes.
    pub async fn poll(&mut self, local_tree: &mut HashTree) {
        // Detect changes
        let mut mutations = local_tree.mutations_from_disk(&self.dir);

        // Return if empty
        if mutations.is_empty() {
            return;
        }

        // Fuse
        Self::fuse_move(&mut mutations, local_tree);

        // Apply
        Self::apply_local_mutations(&self.dir, mutations, local_tree).await;
    }

    /// Fuse [`Mutation`] that correspond to a [`Mutation::Move`].
    pub fn fuse_move(mutations: &mut Vec<Mutation>, local_tree: &HashTree) {
        let mut working_buffer = Vec::new();

        for n_mut in &*mutations {
            let mut copy = None;
            working_buffer.retain(|o_mut| match (n_mut, o_mut) {
                (
                    Mutation::Modify {
                        file_path: m_path,
                        file_hash,
                        timestamp: m_time,
                        ..
                    },
                    Mutation::Remove { file_path: r_path, .. },
                )
                | (
                    Mutation::Remove { file_path: r_path, .. },
                    Mutation::Modify {
                        file_path: m_path,
                        file_hash,
                        timestamp: m_time,
                        ..
                    },
                ) => {
                    if let Some(HashTree::File { hash: r_hash, .. }) = local_tree.get(r_path) {
                        if *r_hash == *file_hash {
                            copy = Some(Mutation::Move {
                                from: r_path.clone(),
                                to: m_path.clone(),
                                timestamp: m_time.clone(),
                            });
                            return false;
                        }
                    }

                    true
                }
                _ => true,
            });

            if let Some(new_mut) = copy {
                working_buffer.push(new_mut);
            } else {
                working_buffer.push(n_mut.clone());
            }
        }

        // Copy the new buffer
        mutations.clear();
        mutations.extend(working_buffer);
    }

    /// Apply [`Mutation`] to a [`SharedDirectory`].
    pub async fn apply_local_mutations(dir: &SharedDirectory, mutations: Vec<Mutation>, local_tree: &mut HashTree) {
        let write_key = match &dir.write_key {
            Some(key) => key,
            None => return,
        };

        let mut inner = dir.write().await;

        for mutation in &mutations {
            match inner.state.mutate(mutation.clone(), write_key) {
                Ok(_) => match local_tree.apply(mutation) {
                    Ok(tree) => {
                        debug!("Applied in {}: {mutation}", dir.uuid());
                        *local_tree = tree;
                    }
                    Err(e) => {
                        error!(
                            "Cannot apply mutation to current tree in {}: {mutation} ({e})",
                            dir.uuid()
                        );
                    }
                },
                Err(e) => {
                    error!("Cannot apply mutation in {}: {mutation} ({e})", dir.uuid());
                }
            }
        }

        //debug!("New state: \n{}", inner.state);
        //debug!("New file tree: \n{}", local_tree);
    }

    pub async fn apply_remote_mutations(&mut self, mutations: Vec<Mutation>, local_tree: &mut HashTree) {
        for mutation in mutations {
            match &mutation {
                Mutation::Modify {
                    file_path,
                    file_hash,
                    file_size,
                    ..
                } => {
                    // Creating DownloadJob
                    let job_ref = Arc::new(RwLock::new(DownloadJob::new(
                        self.dir.uuid,
                        self.dir.path.join(file_path).to_string_lossy().to_string(),
                        *file_hash,
                        *file_size,
                        Utc::now(),
                        JobState::Pending,
                    )));

                    self.downloader.send(DownloaderEvent::Accept(job_ref)).await;
                }
                Mutation::Move { from, to, .. } => {
                    let from = self.dir.path.join(from);
                    let to = self.dir.path.join(to);

                    match fs::rename(&from, &to).await {
                        Ok(_) => self.update_local_tree(mutation, local_tree).await,
                        Err(e) => {
                            error!("Cannot move file: '{}' to '{}' ({e})", from.display(), to.display());
                        }
                    }
                }
                Mutation::Remove { file_path, .. } => {
                    let path = self.dir.path.join(PathBuf::from(file_path));

                    match fs::remove_file(&path).await {
                        Ok(_) => self.update_local_tree(mutation, local_tree).await,
                        Err(e) => {
                            error!("Cannot remove file: '{}' ({e})", path.display());
                        }
                    }
                }
                _ => continue,
            }
        }
    }

    pub fn relative<'a>(dir: &SharedDirectory, file: &'a Path) -> Option<&'a Path> {
        match file.strip_prefix(dir.path.as_path()) {
            Ok(path) => Some(path),
            Err(_) => {
                error!(
                    "Cannot get relative path: '{:?}' in '{:?}'",
                    file.display(),
                    dir.path.display()
                );
                None
            }
        }
    }

    pub(crate) async fn update_local_tree(&self, mutation: Mutation, local_tree: &mut HashTree) {
        match local_tree.apply(&mutation) {
            Ok(tree) => {
                debug!("Applied in {}: {mutation}", self.dir.uuid());
                *local_tree = tree;
            }
            Err(e) => {
                error!(
                    "Cannot apply mutation to local tree in {}: {mutation} ({e})",
                    self.dir.uuid()
                );
            }
        }
    }
}
