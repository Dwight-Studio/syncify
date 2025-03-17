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

                    for node_id in node_id_vec {
                        if !neighbors.keys().any(|e| node_id.as_bytes() == e) {
                            neighbors.insert(*node_id.as_bytes(), true);
                        } else {
                            *neighbors.get_mut(node_id.as_bytes()).unwrap() = true;
                        }
                    }
                    
                    self.handle.send(Event::Sync(SyncEvent::TriggerInitialSync)).await.expect("Unable to push a new event!");
                }
                GossipEvent::NeighborUp(node_id) => {
                    let neighbors = &mut self.dir.inner.write().await.neighbors;

                    if !neighbors.keys().any(|e| node_id.as_bytes() == e) {
                        neighbors.insert(*node_id.as_bytes(), true);
                    } else {
                        *neighbors.get_mut(node_id.as_bytes()).unwrap() = true;
                    }
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
}
