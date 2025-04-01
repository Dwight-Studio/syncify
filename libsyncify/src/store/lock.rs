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
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard, TryLockError};
use crate::store::StoreManager;

#[derive(Clone)]
/// A reader-writer lock used to synchronise persistent data with the store.
pub struct StoreLock<T: Store<G>, G> {
    phantom: PhantomData<(T, G)>,
    store: Arc<RwLock<StoreManager>>,
    inner: Arc<RwLock<T>>,
}

impl<T: Store<G>, G> StoreLock<T, G> {
    pub fn new(store: Arc<RwLock<StoreManager>>, inner: T) -> Self {
        Self {
            phantom: PhantomData,
            store,
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
    pub async fn write(&self) -> G {
        self.inner.write().await.get_guard(self.store.clone())
    }
}

/// Types who can be synchronized with the store.
pub trait Store<G> {
    fn get_guard(&self, store: Arc<RwLock<StoreManager>>) -> G;
}