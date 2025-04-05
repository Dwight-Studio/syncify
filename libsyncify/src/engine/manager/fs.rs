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
use crate::engine::state::{HashTree, Mutation};
use crate::store::lock::StoreLock;
use crate::{SharedDirectory, get_app_cache_dir};
use chrono::Utc;
use log::{debug, error, info};
use std::path::{Path, PathBuf};
use tokio::fs;

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

    /// Apply locally generated [`Mutation`]s to a [`SharedDirectory`].
    pub async fn apply_local_mutations(&self, mutations: Vec<Mutation>) {
        let write_key = match &self.dir.write_key {
            Some(key) => key,
            None => return,
        };

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
                },
                Err(e) => {
                    error!(
                        "Cannot apply mutation to current tree in {}: {mutation} ({e})",
                        self.dir.uuid()
                    );
                }
            }
        }

        self.handle.send(ManagerEvent::BroadcastUpdate).await

        //debug!("New state: \n{}", inner.state);
        //debug!("New file tree: \n{}", local_tree);
    }

    /// Apply remotely generated [`Mutation`]s to a [`SharedDirectory`].
    pub async fn apply_remote_mutations(&mut self, mutations: Vec<Mutation>) {
        for mutation in mutations {
            match &mutation {
                Mutation::Modify { .. } => {
                    // Creating a new job
                    self.downloader
                        .send(DownloaderEvent::Accept(DownloadJob::new(
                            self.dir.uuid,
                            mutation,
                            Utc::now(),
                            JobState::Pending,
                        )))
                        .await;
                }
                Mutation::Move { from, to, .. } => {
                    let from = self.dir.path.join(from);
                    let to = self.dir.path.join(to);

                    match tokio::fs::rename(&from, &to).await {
                        Ok(_) => {
                            // Remove parent (will fail if not empty)
                            let mut parent_opt = from.parent();
                            while let Some(parent) = parent_opt {
                                if parent != self.dir.path() {
                                    let _ = tokio::fs::remove_dir(parent).await;
                                } else {
                                    break;
                                }

                                parent_opt = parent.parent();
                            }

                            self.update_local_tree(mutation).await
                        }
                        Err(e) => {
                            error!("Cannot move file: '{}' to '{}' ({e})", from.display(), to.display());
                        }
                    }
                }
                Mutation::Remove { file_path, .. } => {
                    let path = self.dir.path.join(PathBuf::from(file_path));

                    match fs::remove_file(&path).await {
                        Ok(_) => {
                            // Remove parent (will fail if not empty)
                            let mut parent_opt = path.parent();
                            while let Some(parent) = parent_opt {
                                if parent != self.dir.path() {
                                    let _ = tokio::fs::remove_dir(parent).await;
                                } else {
                                    break;
                                }

                                parent_opt = parent.parent();
                            }
                            
                            self.update_local_tree(mutation).await
                        }
                        Err(e) => {
                            error!("Cannot remove file '{}' ({e})", path.display());
                        }
                    }
                }
                _ => continue,
            }
        }

        self.handle.send(ManagerEvent::BroadcastUpdate).await
    }

    pub fn relative<'a>(dir: &SharedDirectory, file: &'a Path) -> Option<&'a Path> {
        match file.strip_prefix(dir.path.as_path()) {
            Ok(path) => Some(path),
            Err(_) => {
                error!(
                    "Cannot get relative path '{:?}' in '{:?}'",
                    file.display(),
                    dir.path.display()
                );
                None
            }
        }
    }

    pub(crate) async fn download_finished(&self, download_job: StoreLock<DownloadJob>) {
        let job = download_job.read().await;

        let final_path = self.dir.path.join(match job.mutation() {
            Mutation::Modify { file_path, .. } => file_path,
            _ => {
                unreachable!();
            }
        });

        self.update_local_tree(job.mutation().clone()).await;

        info!("Download finished for {}: '{}'", self.dir.uuid, job.path());
        let downloads_dir = get_app_cache_dir().join("downloads");

        if let Some(parent) = final_path.parent() {
            match parent.try_exists() {
                Ok(exists) => {
                    if !exists {
                        tokio::fs::create_dir_all(&parent).await.unwrap();
                    }
                }
                Err(e) => {
                    error!("Cannot check if parent exists {e}")
                }
            }

            if let Err(e) = tokio::fs::rename(downloads_dir.join(job.hash().to_string()), final_path.clone()).await {
                error!(
                    "Cannot copy cache file to '{}' for {} ({e})",
                    final_path.display(),
                    self.dir.uuid,
                );
            }
        } else {
            error!("Cannot get file directory")
        }
    }

    /// Update the local hash tree.
    pub(crate) async fn update_local_tree(&self, mutation: Mutation) {
        
        // Cancel the download of every delete/overwritten file
        let mut files_to_cancel = Vec::new();
        match &mutation {
            Mutation::Modify { file_path: path, .. } | Mutation::Remove { file_path: path, .. } => {
                files_to_cancel.push(path);
            }
            Mutation::Move { from, to, .. } => {
                files_to_cancel.push(from);
                files_to_cancel.push(to);
            }
            _ => ()
        }
        
        let tree = self.dir.local_tree.read().await;
        for file in files_to_cancel {
            if let Some(file) = tree.get(file) {
                self.downloader.send(DownloaderEvent::Cancel(file.hash())).await
            }
        }
        drop(tree);
        
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
