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
use crate::engine::downloader::CHUNK_SIZE;
use crate::engine::job::{DownloadJob, FLUSH_JOB_FREQUENCY, JobState, LocalProvision, RemoteProvision};
use crate::engine::state::{Delta, HashTree, Mutation, State, StateError};
use crate::store::{
    HEAD_TABLE, JOBS_TABLE, LOCAL_PROVISIONS_TABLE, LOCAL_TREE_TABLE, NEIGHBORS_TABLE, REMOTE_PROVISIONS_TABLE,
    StoreError, StoreManager,
};
use crate::{LocalProvisionsMap, NeighborsMap, ReadKey, RemoteProvisionsMap, WriteKey};
use chrono::Utc;
use iroh_base::NodeId;
use log::{debug, error, info};
use redb::TableDefinition;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use tokio::sync::{RwLock, RwLockReadGuard, TryLockError};
use uuid::Uuid;

/// A reader-writer lock used to synchronise persistent data with the store.
pub struct StoreLock<T: Store> {
    store: Weak<RwLock<StoreManager>>,
    inner: Arc<RwLock<T>>,
    id: T::Id,
}

impl<T: Store> StoreLock<T> {
    pub fn new(store: &Arc<RwLock<StoreManager>>, inner: T, id: T::Id) -> Self {
        Self {
            store: Arc::downgrade(&store),
            inner: Arc::new(RwLock::new(inner)),
            id,
        }
    }

    /// Locks this `StoreLock` with shared read access, causing the current task
    /// to yield until the lock has been acquired.
    ///
    /// See [`RwLock::read`] for more details.
    pub async fn read(&self) -> RwLockReadGuard<'_, T> {
        self.inner.read().await
    }

    /// Attempts to acquire this `StoreLock` with shared read access.
    ///
    /// See [`RwLock::try_read`] for more details.
    pub fn try_read(&self) -> Result<RwLockReadGuard<'_, T>, TryLockError> {
        self.inner.try_read()
    }

    /// Locks this `StoreLock` with exclusive write access, causing the current
    /// task to yield until the lock has been acquired. When the lock is
    /// released, the store is updated.
    ///
    /// See [`RwLock::write`] for more details.
    pub fn write(&self) -> StoreGuard<T> {
        if let Some(rf) = self.store.upgrade() {
            T::get_guard(self.inner.clone(), rf, self.id.clone())
        } else {
            panic!("Store lock was dropped");
        }
    }
}

impl<T: Store> Clone for StoreLock<T> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            inner: self.inner.clone(),
            id: self.id.clone(),
        }
    }
}

/// Store guard for [`State`].
pub struct StoreGuard<S: Store> {
    inner: Arc<RwLock<S>>,
    store: Arc<RwLock<StoreManager>>,
    id: S::Id,
}

/// Types who can be synchronized with the store.
pub trait Store: Sized {
    type Id: Clone + Sized;

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self>;
}

impl Store for State {
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self> {
        StoreGuard { inner, store, id }
    }
}

impl StoreGuard<State> {
    /// Completely flush the [`State`]. This should be avoided (use one of the mutation methods instead).
    pub async fn flush(&self) -> Result<(), StateError> {
        let state = self.inner.read().await;

        self.save_head(&state).await?;
        self.save_deltas(&state.iter().collect()).await?;

        Ok(())
    }

    /// Apply a mutation on the [`State`].
    pub async fn mutate(&self, mutation: Mutation, write_key: &WriteKey) -> Result<(), StateError> {
        let mut state = self.inner.write().await;
        state.mutate(mutation, write_key)?;

        self.save_head(&state).await?;
        self.save_deltas(&vec![state.head().clone()]).await?;

        Ok(())
    }

    /// Verify the signature and the consistency of another [`State`] against self, and accept all
    /// the [`Delta`]s. The state is also automatically pruned.
    ///
    /// # Return
    ///
    /// Returns a vec of all accepted [`Mutation`]s in chronological order.
    pub async fn verify_accept_all(
        &mut self,
        other_state: State,
        read_key: &ReadKey,
    ) -> Result<Vec<Mutation>, StateError> {
        let mut state = self.inner.write().await;
        let deltas = state.verify_accept_all(other_state, read_key)?;

        self.save_head(&state).await?;
        self.save_deltas(&deltas).await?;

        if state.trim() {
            debug!("Pruned state {}", self.id.to_string());
        }

        Ok(deltas.iter().map(|d| d.mutation()).collect())
    }

    /// Apply a mutation on the [`State`].
    pub async fn accept(&self, delta: Delta) -> Result<(), StateError> {
        let mut state = self.inner.write().await;
        state.accept(delta);

        self.save_head(&state).await?;
        self.save_deltas(&vec![state.head().clone()]).await?;

        Ok(())
    }

    /// Save the head in the store.
    async fn save_head(&self, state: &State) -> Result<(), StateError> {
        let transaction = self
            .store
            .write()
            .await
            .get_write_transaction()
            .map_err(StateError::Store)?;
        {
            let mut head_table = transaction
                .open_table(HEAD_TABLE)
                .map_err(StoreError::Table)
                .map_err(StateError::Store)?;
            head_table
                .insert(self.id.as_bytes(), state.hash().as_bytes())
                .map_err(StoreError::Storage)
                .map_err(StateError::Store)?;
        }

        transaction
            .commit()
            .map_err(StoreError::Commit)
            .map_err(StateError::Store)
    }

    /// Save a list of [`Delta`]s in the store.
    async fn save_deltas(&self, deltas: &Vec<Arc<Delta>>) -> Result<(), StateError> {
        let transaction = self
            .store
            .write()
            .await
            .get_write_transaction()
            .map_err(StateError::Store)?;
        {
            let uuid_string = self.id.to_string();
            let state_table_def: TableDefinition<[u8; 32], Delta> = TableDefinition::new(&uuid_string);
            let mut state_table = transaction
                .open_table(state_table_def)
                .map_err(StoreError::Table)
                .map_err(StateError::Store)?;

            for delta in deltas {
                state_table
                    .insert(delta.hash().as_bytes(), delta.as_ref())
                    .map_err(StoreError::Storage)
                    .map_err(StateError::Store)?;
            }
        }

        transaction
            .commit()
            .map_err(StoreError::Commit)
            .map_err(StateError::Store)
    }
}

impl Store for HashTree {
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self> {
        StoreGuard { inner, store, id }
    }
}

impl StoreGuard<HashTree> {
    /// Completely flush the [`HashTree`]. This should be avoided (use one of the mutation methods instead).
    pub async fn flush(&self) -> Result<(), StateError> {
        let tree = self.inner.read().await;

        let transaction = self
            .store
            .write()
            .await
            .get_write_transaction()
            .map_err(StateError::Store)?;
        {
            let mut local_tree_table = transaction
                .open_table(LOCAL_TREE_TABLE)
                .map_err(StoreError::Table)
                .map_err(StateError::Store)?;
            local_tree_table
                .insert(self.id.as_bytes(), tree.deref())
                .map_err(StoreError::Storage)
                .map_err(StateError::Store)?;
        }

        transaction
            .commit()
            .map_err(StoreError::Commit)
            .map_err(StateError::Store)
    }

    /// Construct a mutated version of the [`HashTree`].
    pub async fn apply(&self, mutation: &Mutation) -> Result<(), StateError> {
        let mut tree = self.inner.write().await;
        *tree = tree.apply(mutation)?;

        drop(tree);

        self.flush().await
    }
}

impl Store for NeighborsMap {
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self> {
        StoreGuard { inner, store, id }
    }
}

impl StoreGuard<NeighborsMap> {
    /// Completely flush the [`NeighborsMap`]. This should be avoided (use one of the mutation methods instead).
    pub async fn flush(&self) -> Result<(), StoreError> {
        let map = self.inner.read().await;

        let transaction = self.store.write().await.get_write_transaction()?;
        {
            let mut neighbor_table = transaction.open_table(NEIGHBORS_TABLE).map_err(StoreError::Table)?;
            neighbor_table
                .insert(
                    self.id.as_bytes(),
                    map.keys().map(|n| *n.as_bytes()).collect::<Vec<[u8; 32]>>(),
                )
                .map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)
    }

    /// Update a neighbor status.
    pub async fn update(&self, node_id: &NodeId, up: bool) -> Result<(), StoreError> {
        let mut map = self.inner.write().await;
        map.insert(*node_id, up);

        drop(map);
        self.flush().await
    }

    /// Remove a neighbor.
    pub async fn remove(&self, node_id: &NodeId) -> Result<(), StoreError> {
        let mut map = self.inner.write().await;
        map.remove(node_id.as_bytes());

        drop(map);
        self.flush().await
    }
}

impl Store for LocalProvisionsMap {
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self> {
        StoreGuard { inner, store, id }
    }
}

impl StoreGuard<LocalProvisionsMap> {
    /// Insert a new [`LocalProvision`].
    pub async fn insert(&self, provision: LocalProvision) -> Result<(), StoreError> {
        let mut map = self.inner.write().await;
        map.insert(provision.hash(), provision.clone());

        let transaction = self.store.write().await.get_write_transaction()?;
        {
            let mut local_provision_table = transaction
                .open_multimap_table(LOCAL_PROVISIONS_TABLE)
                .map_err(StoreError::Table)?;

            local_provision_table
                .insert(self.id.as_bytes(), provision)
                .map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)
    }

    /// Remove a [`LocalProvision`].
    pub async fn remove(&self, provision: LocalProvision) -> Result<(), StoreError> {
        let mut map = self.inner.write().await;
        map.insert(provision.hash(), provision.clone());

        let transaction = self.store.write().await.get_write_transaction()?;
        {
            let mut local_provision_table = transaction
                .open_multimap_table(LOCAL_PROVISIONS_TABLE)
                .map_err(StoreError::Table)?;

            local_provision_table
                .remove(self.id.as_bytes(), provision)
                .map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)
    }
}

impl Store for RemoteProvisionsMap {
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self> {
        StoreGuard { inner, store, id }
    }
}

impl StoreGuard<RemoteProvisionsMap> {
    /// Insert a new [`RemoteProvision`].
    pub async fn insert(&self, provision: RemoteProvision) -> Result<(), StoreError> {
        if let Some(node_id) = provision.node_id() {
            let mut map = self.inner.write().await;
            let file_map = map.entry(*provision.hash()).or_insert(HashMap::new());
            file_map.insert(node_id, provision.clone());

            let transaction = self.store.write().await.get_write_transaction()?;
            {
                let mut remote_provisions_table = transaction
                    .open_multimap_table(REMOTE_PROVISIONS_TABLE)
                    .map_err(StoreError::Table)?;

                remote_provisions_table
                    .insert(self.id.as_bytes(), provision)
                    .map_err(StoreError::Storage)?;
            }

            transaction.commit().map_err(StoreError::Commit)?;
        }

        Ok(())
    }

    /// Remove a [`LocalProvision`].
    pub async fn remove(&self, provision: RemoteProvision) -> Result<(), StoreError> {
        if let Some(node_id) = provision.node_id() {
            let mut map = self.inner.write().await;
            let file_map = map.entry(*provision.hash()).or_insert(HashMap::new());
            file_map.remove(&node_id);

            if file_map.is_empty() {
                map.remove(&provision.hash());
            }

            let transaction = self.store.write().await.get_write_transaction()?;
            {
                let mut remote_provisions_table = transaction
                    .open_multimap_table(REMOTE_PROVISIONS_TABLE)
                    .map_err(StoreError::Table)?;

                remote_provisions_table
                    .remove(self.id.as_bytes(), provision)
                    .map_err(StoreError::Storage)?;
            }

            transaction.commit().map_err(StoreError::Commit)?;
        }

        Ok(())
    }
}

impl Store for DownloadJob {
    type Id = ();

    fn get_guard(inner: Arc<RwLock<Self>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoreGuard<Self> {
        StoreGuard { inner, store, id }
    }
}

impl StoreGuard<DownloadJob> {
    /// Completely flush the [`DownloadJob`]. This should be avoided (use one of the mutation methods instead).
    pub async fn flush(&self) -> Result<(), StoreError> {
        let job = self.inner.read().await;

        let transaction = self.store.write().await.get_write_transaction()?;
        {
            let mut jobs_table = transaction.open_table(JOBS_TABLE).map_err(StoreError::Table)?;
            jobs_table
                .insert(job.hash().as_bytes(), job.deref())
                .map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)
    }

    /// Add a chunk to the failed chunk list.
    pub async fn add_failed_chunk(&self, index: u64) {
        self.inner.write().await.failed_chunks.push(index);
    }

    /// Prepare the file buffer for downloading.
    ///
    /// # Return
    ///
    /// Returns `false` if there is an error.
    pub async fn start_download(&self, download_dir: &PathBuf) -> bool {
        let mut job = self.inner.write().await;

        if matches!(*job.state(), JobState::Pending) {
            match File::create(download_dir.join(job.hash().to_string())) {
                Ok(file) => {
                    info!("Starting download of '{}'", job.hash());
                    job.state = JobState::Ongoing;
                    job.file = Some(BufWriter::with_capacity(CHUNK_SIZE * 32, file));
                    true
                }
                Err(e) => {
                    error!("Cannot create cache file ({e})");
                    false
                }
            }
        } else if matches!(*job.state(), JobState::Ongoing) {
            match File::options()
                .create(true)
                .write(true)
                .truncate(false)
                .open(download_dir.join(job.hash().to_string()))
            {
                Ok(file) => {
                    info!("Resuming download of '{}'", job.hash());
                    job.file = Some(BufWriter::with_capacity(CHUNK_SIZE * 32, file));
                    true
                }
                Err(e) => {
                    error!("Cannot create cache file ({e})");
                    false
                }
            }
        } else {
            false
        }
    }

    /// Add one to the last chunk counter.
    pub async fn start_download_chunk(&self) {
        self.inner.write().await.last_chunk += 1;
    }

    /// Write data into the file and flush [`DownloadJob`] in the database.
    ///
    /// # Return
    ///
    /// Returns `false` if there is an error.
    pub async fn finish_download_chunk(&self, chunk_index: u64, data: Vec<u8>) -> bool {
        let mut job = self.inner.write().await;

        let file: &mut BufWriter<File> = if let Some(buf) = &mut job.file {
            buf
        } else {
            error!("Cache file cannot be found!");
            return false;
        };

        if let Err(err) = file.seek(SeekFrom::Start(chunk_index * CHUNK_SIZE as u64)) {
            error!("Cannot seek into the cache file: {err}");
            return false;
        }
        if let Err(err) = file.write_all(&data) {
            error!("Cannot write to the cache file: {err}");
            return false;
        }

        // Handling the job update
        job.chunk_done();

        if job.chunk_done % FLUSH_JOB_FREQUENCY == 0 {
            drop(job);
            if let Err(e) = self.flush().await {
                error!("Cannot flush job ({e})");
            }
        }

        true
    }

    /// Flush the file buffer, and update the [`DownloadJob`] in the database.
    pub async fn finish_download(&self) -> bool {
        let mut job = self.inner.write().await;

        let file: &mut BufWriter<File> = if let Some(buf) = &mut job.file {
            buf
        } else {
            error!("Cache file cannot be found!");
            return false;
        };

        if let Err(e) = file.flush() {
            error!("Cannot flush cache file: {e}");
            return false;
        }

        job.file = None;
        job.state = JobState::Done(Utc::now());

        drop(job);
        if let Err(e) = self.flush().await {
            error!("Cannot flush job ({e})");
        }

        true
    }

    /// Delete the [`DownloadJob`] from the database.
    pub async fn delete(&self) -> Result<(), StoreError> {
        let job = self.inner.read().await;

        let transaction = self.store.write().await.get_write_transaction()?;
        {
            let mut jobs_table = transaction.open_table(JOBS_TABLE).map_err(StoreError::Table)?;
            jobs_table.remove(job.hash().as_bytes()).map_err(StoreError::Storage)?;
        }

        transaction.commit().map_err(StoreError::Commit)
    }
}
