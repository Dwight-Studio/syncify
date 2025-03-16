use crate::SharedDirectory;
use iroh_gossip::net::{Event, GossipEvent, GossipSender};
use log::info;

pub async fn handle_events(dir: &SharedDirectory, topic: &GossipSender, gossip_event: iroh_gossip::net::Event) {
    info!("Dir {}: {:?}", dir.uuid(), gossip_event);
    match gossip_event {
        Event::Gossip(event) => {
            match event {
                GossipEvent::Joined(_) => {}
                GossipEvent::NeighborUp(node_id) => {
                    if !dir.inner.read().await.neighbors.iter().any(|e| node_id.as_bytes() == e) {
                        dir.inner.write().await.neighbors.push(*node_id.as_bytes())
                    }
                }
                GossipEvent::NeighborDown(_) => { }
                GossipEvent::Received(_) => {}
            }
        }
        Event::Lagged => {}
    }
}