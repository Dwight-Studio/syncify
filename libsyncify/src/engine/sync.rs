use crate::engine::actor::SyncEvent;
use crate::SharedDirectory;
use iroh_gossip::net::GossipSender;

pub(crate) struct SyncManager {
    topic: GossipSender,
    dir: SharedDirectory,
}

impl SyncManager {
    pub(crate) fn new(topic: GossipSender, dir: SharedDirectory) -> Self {
        Self { topic, dir }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        // Process events
    }
}
