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

use crate::engine::state::{Delta, HashTree, Mutation, State, StateError};
use crate::store::{HEAD_TABLE, LOCAL_TREE_TABLE, StoreError, StoreManager};
use ed25519_dalek::{SigningKey, VerifyingKey};
use log::debug;
use redb::TableDefinition;
use std::ops::Deref;
use std::sync::{Arc, Weak};
use tokio::sync::{RwLock, RwLockReadGuard, TryLockError};
use uuid::Uuid;

/// A reader-writer lock used to synchronise persistent data with the store.
pub struct StoreLock<T: Store<T>> {
    store: Weak<RwLock<StoreManager>>,
    inner: Arc<RwLock<T>>,
    id: T::Id,
}

impl<T: Store<T>> StoreLock<T> {
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
    pub fn write(&self) -> T::Guard {
        if let Some(rf) = self.store.upgrade() {
            T::get_guard(self.inner.clone(), rf, self.id.clone())
        } else {
            panic!("Store lock was dropped");
        }
    }
}

impl<T: Store<T>> Clone for StoreLock<T> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            inner: self.inner.clone(),
            id: self.id.clone(),
        }
    }
}

/// Types who can be synchronized with the store.
pub trait Store<T> {
    type Guard;
    type Id: Clone;

    fn get_guard(inner: Arc<RwLock<T>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> Self::Guard;
}

impl Store<State> for State {
    type Guard = StoredState;
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<State>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoredState {
        StoredState { inner, store, uuid: id }
    }
}

/// Store guard for [`State`].
pub struct StoredState {
    inner: Arc<RwLock<State>>,
    store: Arc<RwLock<StoreManager>>,
    uuid: Uuid,
}

impl StoredState {
    /// Save the [`State`] after creation.
    pub async fn save_new(&self) -> Result<(), StateError> {
        let state = self.inner.write().await;

        self.save_head(&state).await?;
        self.save_deltas(&vec![state.head().clone()]).await?;

        Ok(())
    }

    /// Apply a mutation on the [`State`].
    pub async fn mutate(&self, mutation: Mutation, write_key: &SigningKey) -> Result<(), StateError> {
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
        dir_uuid: Uuid,
        other_state: State,
        read_key: &VerifyingKey,
    ) -> Result<Vec<Mutation>, StateError> {
        let mut state = self.inner.write().await;
        let deltas = state.verify_accept_all(other_state, read_key)?;

        self.save_head(&state).await?;
        self.save_deltas(&deltas).await?;

        if state.trim() {
            debug!("Pruned state {}", dir_uuid.to_string());
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
                .insert(self.uuid.as_bytes(), state.hash().as_bytes())
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
            let uuid_string = self.uuid.to_string();
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

impl Store<HashTree> for HashTree {
    type Guard = StoredHashTree;
    type Id = Uuid;

    fn get_guard(inner: Arc<RwLock<HashTree>>, store: Arc<RwLock<StoreManager>>, id: Self::Id) -> StoredHashTree {
        StoredHashTree { inner, store, uuid: id }
    }
}

/// Store guard for [`State`].
pub struct StoredHashTree {
    inner: Arc<RwLock<HashTree>>,
    store: Arc<RwLock<StoreManager>>,
    uuid: Uuid,
}

impl StoredHashTree {
    /// Save the [`State`] after creation.
    pub async fn save_new(&self) -> Result<(), StateError> {
        let tree = self.inner.write().await;
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
                .insert(self.uuid.as_bytes(), tree.deref())
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

        self.save_new().await
    }
}
