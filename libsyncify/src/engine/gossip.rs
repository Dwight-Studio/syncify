use crate::SharedDirectory;
use iroh_gossip::net::GossipSender;
use log::info;

pub async fn handle_events(dir: &SharedDirectory, topic: &GossipSender, gossip_event: iroh_gossip::net::Event) {
    info!("Dir {}: {:?}", dir.uuid(), gossip_event);
}