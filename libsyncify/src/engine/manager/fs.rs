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
use crate::engine::downloader::{DownloaderEvent, DownloaderHandle};
use crate::engine::job::{DownloadJob, JobState};
use crate::engine::manager::{ManagerEvent, ManagerHandle};
use crate::engine::state::{HashTree, MAX_LOADED_DELTAS, Mutation};
use crate::{SharedDirectory, get_app_cache_dir};
use chrono::Utc;
use log::{debug, error};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;

pub struct FileSystemManager {
    dir: SharedDirectory,
    handle: ManagerHandle,
    downloader: DownloaderHandle,
}

impl FileSystemManager {
    pub async fn new(dir: SharedDirectory, handle: ManagerHandle, downloader: DownloaderHandle) -> Self {
        Self {
            dir,
            handle,
            downloader,
        }
    }

    /// Poll the file system for changes.
    pub async fn poll(&mut self) {
        // Detect changes
        let mut mutations = self.dir.local_tree.read().await.mutations_from_disk(&self.dir);

        // Return if empty
        if mutations.is_empty() {
            return;
        }

        // Fuse
        self.fuse_move(&mut mutations).await;

        // Apply
        self.apply_local_mutations(mutations).await;
    }

    /// Fuse [`Mutation`] that correspond to a [`Mutation::Move`].
    pub async fn fuse_move(&self, mutations: &mut Vec<Mutation>) {
        let local_tree = self.dir.local_tree.read().await;
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
                        if r_hash == file_hash {
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
    pub async fn apply_local_mutations(&self, mutations: Vec<Mutation>) {
        let write_key = match &self.dir.write_key {
            Some(key) => key,
            None => return,
        };

        let prev_head = self.dir.state.read().await.hash();
        for mutation in &mutations {
            match self.dir.state.write().mutate(mutation.clone(), write_key).await {
                Ok(_) => match self.dir.local_tree.write().apply(mutation).await {
                    Ok(_) => {
                        debug!("Applied in {}: {mutation}", self.dir.uuid());
                    }
                    Err(e) => {
                        error!(
                            "Cannot apply mutation to current tree in {}: {mutation} ({e})",
                            self.dir.uuid()
                        );
                    }
                }
                Err(e) => {
                    error!(
                        "Cannot apply mutation to current tree in {}: {mutation} ({e})",
                        self.dir.uuid()
                    );
                }
            }
        }
        
        if let Some(state) = self.dir.state.read().await.clone_after(prev_head, MAX_LOADED_DELTAS) {
            self.handle.send(ManagerEvent::BroadcastChange(state)).await;
        } else {
            error!("State is now invalid (unable to find previous head)");
        }

        //debug!("New state: \n{}", inner.state);
        //debug!("New file tree: \n{}", local_tree);
    }

    pub async fn apply_remote_mutations(&mut self, mutations: Vec<Mutation>) {
        for mutation in mutations {
            match &mutation {
                Mutation::Modify { .. } => {
                    // Creating DownloadJob
                    let job_ref = Arc::new(RwLock::new(DownloadJob::new(
                        self.dir.uuid,
                        mutation,
                        Utc::now(),
                        JobState::Pending,
                    )));

                    self.downloader.send(DownloaderEvent::Accept(job_ref)).await;
                }
                Mutation::Move { from, to, .. } => {
                    let from = self.dir.path.join(from);
                    let to = self.dir.path.join(to);

                    match fs::rename(&from, &to).await {
                        Ok(_) => self.update_local_tree(mutation).await,
                        Err(e) => {
                            error!("Cannot move file: '{}' to '{}' ({e})", from.display(), to.display());
                        }
                    }
                }
                Mutation::Remove { file_path, .. } => {
                    let path = self.dir.path.join(PathBuf::from(file_path));

                    match fs::remove_file(&path).await {
                        Ok(_) => self.update_local_tree(mutation).await,
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

    pub(crate) async fn download_finished(&self, download_job: Arc<RwLock<DownloadJob>>) {
        let job = download_job.read().await;

        let final_path = self.dir.path.join(match job.mutation() {
            Mutation::Modify { file_path, .. } => file_path,
            _ => {
                unreachable!();
            }
        });

        self.update_local_tree(job.mutation().clone()).await;

        debug!("Download finished, copying cache file into directory...");
        let downloads_dir = get_app_cache_dir().join("downloads");

        if let Err(e) = tokio::fs::copy(downloads_dir.join(job.hash().to_string()), final_path.clone()).await {
            error!(
                "Cannot copy cache file to '{}' for {} ({e})",
                final_path.display(),
                self.dir.uuid,
            );
        }
    }

    pub(crate) async fn update_local_tree(&self, mutation: Mutation) {
        match self.dir.local_tree.write().apply(&mutation).await {
            Ok(_) => {
                debug!("Applied in {}: {mutation}", self.dir.uuid());
            }
            Err(e) => {
                error!(
                    "Cannot apply mutation to current tree in {}: {mutation} ({e})",
                    self.dir.uuid()
                );
            }
        }
    }
}
