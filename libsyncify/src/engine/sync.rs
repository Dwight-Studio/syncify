use crate::engine::actor::SyncEvent;
use crate::SharedDirectory;
use iroh_gossip::net::GossipSender;

pub async fn handle_events(dir: &SharedDirectory, topic: &GossipSender, sync_event: SyncEvent) {
    // Process events
}