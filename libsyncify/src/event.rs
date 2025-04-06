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
use crate::engine::state::State;
use iroh_base::NodeId;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Size of the [`SyncifyEvent`] for [`Notifier`].
pub const EVENT_BUFFER_SIZE: usize = 1024;

pub type EventSender = broadcast::Sender<SyncifyEvent>;
pub type EventReceiver = broadcast::Receiver<SyncifyEvent>;

/// Hierarchical event type providing layered precision.
#[derive(Debug, Clone)]
pub enum SyncifyEvent {
    Engine(EngineEvent),
    Directory(DirectoryEvent),
}

/// [`Engine`] related events.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// [`SharedDirectory`] manager started (synchronization is now active for that directory).
    ManagerStarted(Uuid),
    /// [`SharedDirectory`] manager no longer takes new events (synchronization is now inactive for that directory).
    ManagerClosed(Uuid),
    /// [`SharedDirectory`] manager has processed all events.
    ManagerStopped(Uuid),
    /// [`Downloader`] started (content downloading is now available).
    DownloaderStarted,
    /// [`Downloader`] no longer takes new events (content downloading is no longer available).
    DownloaderClosed,
    /// [`Downloader`] manager has processed all events.
    DownloaderStopped,
}

/// [`SharedDirectory`] related events.
#[derive(Debug, Clone)]
pub enum DirectoryEvent {
    /// A [`SharedDirectory`] has been created.
    Created(SharedDirectory),
    /// A [`SharedDirectory`] has been joined.
    Joined(SharedDirectory),
    /// A [`SharedDirectory`] has been removed.
    Removed(SharedDirectory),
    /// A [`SharedDirectory`] has received synchronisation information.
    Sync(Uuid, Box<SyncEvent>),
}

/// [`SharedDirectory`] synchronization related events.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// A request for synchronization has been received.
    Incoming(NodeId),
    /// A request for synchronization has been sent.
    Outgoing(NodeId),
    /// Conflict detected during synchronization with a peer.
    Conflict(NodeId, ConflictEvent),
    /// Unverified changes detected during synchronization with a peer.
    Unverified(NodeId),
    /// The directory has been updated with locally generated changes.
    Local(State),
    /// The directory has been updated with remotely generated changes.
    Remote(State),
}

/// Synchronization conflicts related events.
#[derive(Debug, Clone)]
pub enum ConflictEvent {
    /// A conflict has been detected but the changes didn't overlap.
    /// The conflict was automatically resolved.
    Trivial,
    /// A conflict has been detected and the changes overlap.
    /// The conflict require user intervention to be resolved.
    RequireIntervention { local: State, remote: State, callback: () },
    /// A conflict has been detected and the changes overlap.
    /// The conflict has been resolved by remote peer.
    Resolved,
    /// A conflict has been detected and the changes overlap.
    /// The conflict resolution has been cancelled.
    Cancelled,
    /// A conflict has been detected and the changes overlap.
    /// None of the peers involved in the synchronization can resolve the conflict (requiring write permission).
    Stalemate,
}
