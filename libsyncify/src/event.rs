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
use crate::engine::job::{LocalProvision, RemoteProvision};
use crate::engine::state::State;
use blake3::Hash;
use iroh_base::NodeId;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Size of the [`SyncifyEvent`] buffer.
pub const EVENT_BUFFER_SIZE: usize = 1024;

pub type EventReceiver = broadcast::Receiver<SyncifyEvent>;

// TODO: Add connection/peer event
// TODO: Add download event

#[derive(Debug, Clone)]
pub struct EventSender {
    sender: broadcast::Sender<SyncifyEvent>,
}

impl Default for EventSender {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSender {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_BUFFER_SIZE);

        EventSender { sender }
    }

    /// Create a new [`EventReceiver`] linked to this sender.
    pub fn subscribe(&self) -> EventReceiver {
        self.sender.subscribe()
    }

    /// Send an event to all [`EventReceiver`].
    pub fn send(&self, event: SyncifyEvent) {
        if let Err(_e) = self.sender.send(event) {
            // Ignore, it just means that there no receiver
        }
    }
}

/// Hierarchical event type providing layered precision.
#[derive(Debug, Clone)]
pub enum SyncifyEvent {
    Engine(EngineEvent),
    Directory(Uuid, DirectoryEvent),
    Download(DownloadEvent),
}

/// [`Engine`] related events.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Directory manager started (synchronization is now active for that directory).
    ManagerStarted(Uuid),
    /// Directory manager no longer takes new events (synchronization is now inactive for that directory).
    ManagerStopped(Uuid),
    /// Directory manager has processed all events.
    ManagerFinished(Uuid),
    /// [`Downloader`] started (content downloading is now available).
    DownloaderStarted,
    /// [`Downloader`] no longer takes new events (content downloading is no longer available).
    DownloaderStopped,
    /// [`Downloader`] manager has processed all events.
    DownloaderFinished,
}

impl EngineEvent {
    /// Recursively wrap this event into a [`SyncifyEvent`].
    pub fn wrap(self) -> SyncifyEvent {
        SyncifyEvent::Engine(self)
    }
}

/// [`SharedDirectory`] related events.
#[derive(Debug, Clone)]
pub enum DirectoryEvent {
    /// A [`SharedDirectory`] has been created.
    Created,
    /// A [`SharedDirectory`] has been joined.
    Joined,
    /// A [`SharedDirectory`] has been removed.
    Removed,
    /// A directory has received synchronisation information.
    Sync(Box<SyncEvent>),
    /// A directory has received peer status update.
    Peer(PeerEvent),
    
}

impl DirectoryEvent {
    /// Recursively wrap this event into a [`SyncifyEvent`].
    pub fn wrap(self, uuid: Uuid) -> SyncifyEvent {
        SyncifyEvent::Directory(uuid, self)
    }
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

impl SyncEvent {
    /// Recursively wrap this event into a [`SyncifyEvent`].
    pub fn wrap(self, uuid: Uuid) -> SyncifyEvent {
        DirectoryEvent::Sync(Box::new(self)).wrap(uuid)
    }
}

/// Synchronization conflicts related events.
#[derive(Debug, Clone)]
pub enum ConflictEvent {
    /// A conflict has been detected but the changes didn't overlap.
    /// The conflict was automatically resolved.
    Trivial, //TODO
    /// A conflict has been detected and the changes overlap.
    /// The conflict require user intervention to be resolved.
    RequireIntervention { local: State, remote: State, callback: () }, //TODO
    /// A conflict has been detected and the changes overlap.
    /// The conflict has been resolved by remote peer.
    Resolved, //TODO
    /// A conflict has been detected and the changes overlap.
    /// The conflict resolution has been cancelled.
    Cancelled, //TODO
    /// A conflict has been detected and the changes overlap.
    /// None of the peers involved in the synchronization can resolve the conflict (requiring write permission).
    Stalemate, //TODO
}

impl ConflictEvent {
    /// Recursively wrap this event into a [`SyncifyEvent`].
    pub fn wrap(self, uuid: Uuid, node_id: NodeId) -> SyncifyEvent {
        SyncEvent::Conflict(node_id, self).wrap(uuid)
    }
}

/// [`Downloader`] related events.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// A [`LocalProvision`] has been updated (a file is now ready to be uploaded to other peers).
    LocalProvisionUpdate(LocalProvision),
    /// A [`RemoteProvision`] has been updated (a file is now ready to be downloaded from another peer).
    RemoteProvisionUpdate(RemoteProvision),
    /// A file is now being uploaded to another peer.
    UploadStarted { file_hash: Hash, peer_node_id: NodeId }, //TODO
    /// A file is no longer being upload to another peer.
    UploadStopped { file_hash: Hash, peer_node_id: NodeId }, //TODO
    /// A file (for a specific directory) is now being downloaded from another peer.
    DownloadStarted { dir_uuid: Uuid, file_hash: Hash },
    /// The download of a file (for a specific directory) has progressed.
    /// More specifically, a chunk of the file has been successfully download from another peer.
    DownloadProgressed {
        dir_uuid: Uuid,
        file_hash: Hash,
        progress: f32,
        download_chunk_index: u64,
        peer_node_id: NodeId,
    },
    /// The download of a file (for a specific directory) has failed.
    DownloadCancelled { dir_uuid: Uuid, file_hash: Hash },
    /// The download of a file (for a specific directory) has been completed.
    DownloadCompleted { dir_uuid: Uuid, file_hash: Hash },
}

impl DownloadEvent {
    /// Recursively wrap this event into a [`SyncifyEvent`].
    pub fn wrap(self) -> SyncifyEvent {
        SyncifyEvent::Download(self)
    }
}

/// Peers related events.
#[derive(Debug, Clone)]
pub enum PeerEvent {
    /// A peer from the directory swarm is now connected.
    Up(NodeId),
    /// A peer from the directory swarm is now disconnected.
    Down(NodeId),
}

impl PeerEvent {
    /// Recursively wrap this event into a [`SyncifyEvent`].
    pub fn wrap(self, uuid: Uuid) -> SyncifyEvent {
        DirectoryEvent::Peer(self).wrap(uuid)
    }
}
