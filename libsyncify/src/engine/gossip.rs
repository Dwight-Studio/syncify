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
use std::collections::HashMap;
use iroh::NodeId;
use crate::engine::actor::{DirectoryManagerHandle, Event, SyncEvent};
use crate::SharedDirectory;
use iroh_gossip::net::{GossipEvent, GossipSender};
use log::info;

pub(crate) struct GossipManager {
    topic: GossipSender,
    dir: SharedDirectory,
    handle: DirectoryManagerHandle
}

impl GossipManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, handle: DirectoryManagerHandle) -> Self {
        Self { topic, dir, handle }
    }

    pub(crate) async fn handle_events(&mut self, gossip_event: iroh_gossip::net::Event) {
        info!("Dir {}: {:?}", self.dir.uuid(), gossip_event);
        match gossip_event {
            iroh_gossip::net::Event::Gossip(event) => match event {
                GossipEvent::Joined(node_id_vec) => {
                    let neighbors = &mut self.dir.inner.write().await.neighbors;

                    for node_id in &node_id_vec {
                        Self::update_neighbors(neighbors, node_id);
                    }
                    
                    self.handle.send(Event::Sync(SyncEvent::TriggerSync(None))).await.expect("Unable to push a new event!");
                }
                GossipEvent::NeighborUp(node_id) => {
                    let neighbors = &mut self.dir.inner.write().await.neighbors;

                    Self::update_neighbors(neighbors, &node_id);
                }
                GossipEvent::NeighborDown(node_id) => {
                    let neighbors = &mut self.dir.inner.write().await.neighbors;
                    *neighbors.get_mut(node_id.as_bytes()).unwrap() = false;
                }
                GossipEvent::Received(_) => {}
            },
            iroh_gossip::net::Event::Lagged => {}
        }
    }

    fn update_neighbors(neighbors: &mut HashMap<[u8; 32], bool>, node_id: &NodeId) {
        if !neighbors.keys().any(|e| node_id.as_bytes() == e) {
            neighbors.insert(*node_id.as_bytes(), true);
        } else {
            *neighbors.get_mut(node_id.as_bytes()).unwrap() = true;
        }
    }
}
