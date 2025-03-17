use crate::SharedDirectory;
use iroh_gossip::net::{Event, GossipEvent, GossipSender};
use log::info;

pub(crate) struct GossipManager {
    topic: GossipSender,
    dir: SharedDirectory,
}

impl GossipManager {
    pub(crate) fn new(topic: GossipSender, dir: SharedDirectory) -> Self {
        Self { topic, dir }
    }

    pub(crate) async fn handle_events(&mut self, gossip_event: iroh_gossip::net::Event) {
        info!("Dir {}: {:?}", self.dir.uuid(), gossip_event);
        match gossip_event {
            Event::Gossip(event) => match event {
                GossipEvent::Joined(_) => {}
                GossipEvent::NeighborUp(node_id) => {
                    let neighbors = &mut self.dir.inner.write().await.neighbors;

                    if !neighbors.iter().any(|e| node_id.as_bytes() == e) {
                        neighbors.push(*node_id.as_bytes())
                    }
                }
                GossipEvent::NeighborDown(_) => {}
                GossipEvent::Received(_) => {}
            },
            Event::Lagged => {}
        }
    }
}
