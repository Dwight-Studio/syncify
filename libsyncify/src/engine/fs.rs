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
use crate::engine::state::{HashTree, Mutation};
use blake3::Hash;
use chrono::{DateTime, TimeDelta, Utc};
use iroh_gossip::net::GossipSender;
use log::{error, info};
use notify::EventKind::{Modify, Remove};
use notify::event::{ModifyKind, RemoveKind, RenameMode};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::sync::RwLock;

pub struct FileSystemManager {
    topic: GossipSender,
    dir: SharedDirectory,
    mutations_buffer: Vec<Mutation>,
}

impl FileSystemManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, last_tree: &mut HashTree) -> Self {
        let inner = dir.read().await;

        let mut local_buffer = if dir.is_read_only() {
            Vec::new()
        } else {
            inner.state.hash_tree().mutations_from_disk(&dir)
        };

        Self::clean_mutation_buffer(&mut local_buffer, Some(&last_tree));

        Self {
            topic,
            dir: dir.clone(),
            mutations_buffer: local_buffer,
        }
    }

    pub(crate) async fn handle_events(&mut self, fs_event: notify::Event, timestamp: DateTime<Utc>, jobs_buffer: &mut Vec<Arc<RwLock<Job>>>, last_tree: &mut HashTree) {
        let mut hasher = blake3::Hasher::new();

        match fs_event.kind {
            Modify(ModifyKind::Name(RenameMode::Both)) => {
                if let (Some(path_from), Some(path_to)) = (
                    relative(&self.dir, &fs_event.paths[0]),
                    relative(&self.dir, &fs_event.paths[1]),
                ) {
                    // Checking if it correspondes to a job
                    for job_ref in jobs_buffer.clone() {
                        let job = job_ref.read().await;
                        if let Job::Move {
                            from,
                            to,
                            state: JobState::Done(job_timestamp),
                            ..
                        } = job.deref()
                        {
                            if from == path_from
                                && to == path_to
                                && job_timestamp
                                    .signed_duration_since(timestamp)
                                    .abs()
                                    .le(&TimeDelta::new(1, 0).unwrap())
                            {
                                return;
                            }
                        }
                    }

                    info!("File {:?} moved to {:?} in {}", path_from, path_to, self.dir.uuid());

                    let mutation = Mutation::Move {
                        from: path_from.to_string_lossy().to_string(),
                        to: path_to.to_string_lossy().to_string(),
                        timestamp,
                    };

                    self.mutations_buffer.push(mutation);
                }
            }
            Modify(ModifyKind::Data(_)) | Modify(ModifyKind::Name(RenameMode::To)) => {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(&self.dir, &abs_path) {
                        // Checking if it correspondes to a job
                        for job_ref in jobs_buffer.clone() {
                            let job = job_ref.read().await;
                            if let Job::Move {
                                to,
                                state: JobState::Done(job_timestamp),
                                ..
                            } = job.deref()
                            {
                                if to == path && job_timestamp
                                        .signed_duration_since(timestamp)
                                        .abs()
                                        .le(&TimeDelta::new(1, 0).unwrap()) {
                                    return;
                                }
                            }
                            if let Job::Download {
                                path: job_path,
                                state: JobState::Done(job_timestamp),
                                ..
                            } = job.deref()
                            {
                                if job_path == path && job_timestamp
                                    .signed_duration_since(timestamp)
                                    .abs()
                                    .le(&TimeDelta::new(1, 0).unwrap()) {
                                    return;
                                }
                            }
                        }

                        info!("File '{:?}' modified in {}", path.display(), self.dir.uuid());
                        let file_hash = match hasher.update_mmap(&abs_path) {
                            Ok(hash) => *hash.finalize().as_bytes(),
                            Err(_) => {
                                error!("Cannot compute hash: {}", path.display());
                                continue;
                            }
                        };

                        let mutation = Mutation::Modify {
                            file_path: path.to_string_lossy().to_string(),
                            file_hash: Hash::from(file_hash),
                            timestamp,
                        };

                        self.mutations_buffer.push(mutation);
                    }
                }
            }
            Remove(RemoveKind::File) | Modify(ModifyKind::Name(RenameMode::From)) => {
                for abs_path in fs_event.paths {
                    if let Some(path) = relative(&self.dir, &abs_path) {
                        // Checking if it correspondes to a job
                        for job_ref in jobs_buffer.clone() {
                            let job = job_ref.read().await;
                            if let Job::Move {
                                from,
                                state: JobState::Done(job_timestamp),
                                ..
                            } = job.deref()
                            {
                                if from == path && job_timestamp
                                    .signed_duration_since(timestamp)
                                    .abs()
                                    .le(&TimeDelta::new(1, 0).unwrap()) {
                                    return;
                                }
                            }
                            if let Job::Remove {
                                path: job_path,
                                state: JobState::Done(job_timestamp),
                                ..
                            } = job.deref()
                            {
                                if job_path == path && job_timestamp
                                    .signed_duration_since(timestamp)
                                    .abs()
                                    .le(&TimeDelta::new(1, 0).unwrap()) {
                                    return;
                                }
                            }
                        }
                        
                        info!("File '{:?}' removed in {}", path.display(), self.dir.uuid());

                        let mutation = Mutation::Remove {
                            file_path: path.to_string_lossy().to_string(),
                            timestamp,
                        };

                        self.mutations_buffer.push(mutation);
                    }
                }
            }
            _ => return,
        }

        Self::clean_mutation_buffer(&mut self.mutations_buffer, Some(&last_tree));
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
                )
                | (
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
                )
                | (
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

    /// Apply [`Mutation`] in the mutation buffer.
    pub(crate) async fn apply_local_mutations(&mut self, last_tree: &mut HashTree) {
        let write_key = match &self.dir.sign_key {
            Some(key) => key,
            None => return,
        };

        let mut inner = self.dir.write().await;

        info!("Applying mutations for {}", self.dir.uuid());

        for mutation in &self.mutations_buffer {
            match inner.state.mutate(mutation.clone(), write_key) {
                Ok(_) => {
                    info!("Applied: {mutation}");
                }
                Err(e) => {
                    error!("Cannot apply mutation: {mutation} ({e}");
                }
            }
        }

        self.mutations_buffer.clear();

        *last_tree = inner.state.hash_tree().clone();
        info!("\n{}", last_tree)
    }

    pub(crate) async fn generate_jobs(&mut self, jobs_buffer: &mut Vec<Arc<RwLock<Job>>>, mutations: Vec<Mutation>) {
        for mutation in mutations {
            match &mutation {
                Mutation::Modify {
                    file_path, file_hash, ..
                } => {
                    let job_ref = Arc::new(RwLock::new(Job::Download {
                        path: self.dir.path.join(file_path),
                        hash: *file_hash,
                        state: JobState::Pending,
                    }));
                    jobs_buffer.push(job_ref.clone());

                    let job = job_ref.write().await;

                    // TODO: Add download process
                }
                Mutation::Move { from, to, .. } => {
                    let from = self.dir.path.join(from);
                    let to = self.dir.path.join(to);

                    let job_ref = Arc::new(RwLock::new(Job::Move {
                        from: from.clone(),
                        to: to.clone(),
                        state: JobState::Done(Utc::now()),
                    }));
                    jobs_buffer.push(job_ref.clone());

                    match fs::rename(&from, &to).await {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Cannot move file: '{}' to '{}' ({e})", from.display(), to.display());
                            if let Job::Move { state, .. } = job_ref.write().await.deref_mut() {
                                *state = JobState::Error(e.into())
                            }
                        }
                    }
                }
                Mutation::Remove { file_path, .. } => {
                    let path = self.dir.path.join(PathBuf::from(file_path));
                    let job_ref = Arc::new(RwLock::new(Job::Remove {
                        path: path.clone(),
                        state: JobState::Done(Utc::now()),
                    }));
                    jobs_buffer.push(job_ref.clone());

                    match fs::remove_file(&path).await {
                        Ok(_) => {}
                        Err(e) => {
                            error!("Cannot remove file: '{}' ({e})", path.display());
                            if let Job::Remove { state, .. } = job_ref.write().await.deref_mut() {
                                *state = JobState::Error(e.into())
                            }
                        }
                    }
                }
                _ => continue,
            }
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

pub enum Job {
    Download {
        path: PathBuf,
        hash: Hash,
        state: JobState,
    },
    Remove {
        path: PathBuf,
        state: JobState,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
        state: JobState,
    },
}

pub enum JobState {
    Pending,
    Ongoing(f32),
    Done(DateTime<Utc>),
    Error(anyhow::Error),
}
