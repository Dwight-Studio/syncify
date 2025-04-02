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
use crate::engine::state::{Delta, HashTree, Mutation, State, StateError};
use crate::store::{HEAD_TABLE, LOCAL_TREE_TABLE, StoreError, StoreManager};
use ed25519_dalek::{SigningKey, VerifyingKey};
use log::{debug, error};
use redb::{TableDefinition, WriteTransaction};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::{Arc, Weak};
use tokio::sync::{RwLock, RwLockReadGuard, TryLockError};
use uuid::Uuid;

/// A reader-writer lock used to synchronise persistent data with the store.
pub struct StoreLock<T: Store<T, G>, G> {
    marker: PhantomData<(T, G)>,
    store: Weak<RwLock<StoreManager>>,
    inner: Arc<RwLock<T>>,
}

impl<'a, T: Store<T, G>, G> StoreLock<T, G> {
    pub fn new(store: &Arc<RwLock<StoreManager>>, inner: T) -> Self {
        Self {
            marker: PhantomData,
            store: Arc::downgrade(&store),
            inner: Arc::new(RwLock::new(inner)),
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
    pub async fn write(&'a self) -> Result<G, StoreError> {
        if let Some(rf) = self.store.upgrade() {
            T::get_guard(self.inner.clone(), rf).await
        } else {
            panic!("Store lock was dropped");
        }
    }
}

impl<T: Store<T, G>, G> Clone for StoreLock<T, G> {
    fn clone(&self) -> Self {
        Self {
            marker: self.marker,
            store: self.store.clone(),
            inner: self.inner.clone(),
        }
    }
}

/// Types who can be synchronized with the store.
pub trait Store<T, G> {
    fn get_guard(
        inner: Arc<RwLock<T>>,
        store: Arc<RwLock<StoreManager>>,
    ) -> impl Future<Output = Result<G, StoreError>>;
}

impl Store<State, StoredState> for State {
    async fn get_guard(inner: Arc<RwLock<State>>, store: Arc<RwLock<StoreManager>>) -> Result<StoredState, StoreError> {
        Ok(StoredState {
            inner,
            transaction: Some(store.write().await.get_write_transaction().await?),
            dir_uuid: Uuid::from_bytes([0u8; 16]),
        })
    }
}

/// Store guard for [`State`].
pub struct StoredState {
    inner: Arc<RwLock<State>>,
    transaction: Option<WriteTransaction>,
    dir_uuid: Uuid,
}

impl StoredState {
    pub fn set_dir(&mut self, dir: &SharedDirectory) {
        self.dir_uuid = dir.uuid();
    }

    /// Save the [`State`] after creation.
    pub async fn save_new_state(&self) -> Result<(), StateError> {
        let state = self.inner.write().await;

        self.save_head(&state)?;
        self.save_deltas(&vec![state.head().clone()])?;

        Ok(())
    }

    /// Apply a mutation on the [`State`].
    pub async fn mutate(&self, mutation: Mutation, write_key: &SigningKey) -> Result<(), StateError> {
        let mut state = self.inner.write().await;
        state.mutate(mutation, write_key)?;

        self.save_head(&state)?;
        self.save_deltas(&vec![state.head().clone()])?;

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
        read_key: &VerifyingKey,
    ) -> Result<Vec<Mutation>, StateError> {
        let mut state = self.inner.write().await;
        let deltas = state.verify_accept_all(other_state, read_key)?;

        self.save_head(&state)?;
        self.save_deltas(&deltas)?;

        if state.trim() {
            debug!("Pruned state {}", self.dir_uuid.to_string());
        }

        Ok(deltas.iter().map(|d| d.mutation()).collect())
    }

    /// Apply a mutation on the [`State`].
    pub async fn accept(&self, delta: Delta) -> Result<(), StateError> {
        let mut state = self.inner.write().await;
        state.accept(delta);

        self.save_head(&state)?;
        self.save_deltas(&vec![state.head().clone()])?;

        Ok(())
    }

    /// Save the head in the store.
    fn save_head(&self, state: &State) -> Result<(), StateError> {
        if let Some(transaction) = self.transaction.as_ref() {
            let mut head_table = transaction
                .open_table(HEAD_TABLE)
                .map_err(StoreError::Table)
                .map_err(StateError::Store)?;
            head_table
                .insert(self.dir_uuid.as_bytes(), state.hash().as_bytes())
                .map_err(StoreError::Storage)
                .map_err(StateError::Store)?;
        } else {
            panic!("Missing transaction");
        }
        Ok(())
    }

    /// Save a list of [`Delta`]s in the store.
    fn save_deltas(&self, deltas: &Vec<Arc<Delta>>) -> Result<(), StateError> {
        if let Some(transaction) = self.transaction.as_ref() {
            let uuid_string = self.dir_uuid.to_string();
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
        } else {
            panic!("Missing transaction");
        }
        Ok(())
    }
}

impl Drop for StoredState {
    fn drop(&mut self) {
        if let Err(e) = self.transaction.take().unwrap().commit() {
            error!("Failed to commit transaction: {:?}", e);
        }
    }
}

impl Store<HashTree, StoredHashTree> for HashTree {
    async fn get_guard(
        inner: Arc<RwLock<HashTree>>,
        store: Arc<RwLock<StoreManager>>,
    ) -> Result<StoredHashTree, StoreError> {
        Ok(StoredHashTree {
            inner,
            transaction: Some(store.write().await.get_write_transaction().await?),
            dir_uuid: Uuid::from_bytes([0u8; 16]),
        })
    }
}

/// Store guard for [`State`].
pub struct StoredHashTree {
    inner: Arc<RwLock<HashTree>>,
    transaction: Option<WriteTransaction>,
    dir_uuid: Uuid,
}

impl StoredHashTree {
    pub fn set_dir(&mut self, dir: &SharedDirectory) {
        self.dir_uuid = dir.uuid();
    }

    /// Save the [`State`] after creation.
    pub async fn save_new_state(&self) -> Result<(), StateError> {
        let tree = self.inner.write().await;

        if let Some(transaction) = self.transaction.as_ref() {
            let mut local_tree_table = transaction
                .open_table(LOCAL_TREE_TABLE)
                .map_err(StoreError::Table)
                .map_err(StateError::Store)?;
            local_tree_table
                .insert(self.dir_uuid.as_bytes(), tree.deref())
                .map_err(StoreError::Storage)
                .map_err(StateError::Store)?;
        }

        Ok(())
    }

    /// Construct a mutated version of the [`HashTree`].
    pub async fn apply(&self, mutation: &Mutation) -> Result<(), StateError> {
        let mut tree = self.inner.write().await;
        *tree = tree.apply(mutation)?;

        drop(tree);

        self.save_new_state().await
    }
}

impl Drop for StoredHashTree {
    fn drop(&mut self) {
        if let Err(e) = self.transaction.take().unwrap().commit() {
            error!("Failed to commit transaction: {:?}", e);
        }
    }
}
