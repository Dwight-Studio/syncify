use crate::engine::actor::SyncEvent;
use crate::engine::protocol::{SyncifyPacket, SyncifyProtocol};
use crate::SharedDirectory;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305};
use iroh::NodeId;
use iroh_gossip::net::GossipSender;
use log::{error, info};

pub(crate) struct SyncManager {
    pub(crate) topic: GossipSender,
    pub(crate) dir: SharedDirectory,
    pub(crate) syncify_prot: SyncifyProtocol
}

impl SyncManager {
    pub(crate) async fn new(topic: GossipSender, dir: SharedDirectory, syncify_prot: SyncifyProtocol) -> Self {
        Self { topic, dir, syncify_prot }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        match sync_event { 
            SyncEvent::RequestDeltas(mut conn) => {
                let packet = SyncifyPacket::Failed;
                
                conn.send_packet(self.dir.clone(), packet).await;
            },
            SyncEvent::TriggerInitialSync => {
                self.initial_sync().await;
            }
        }
    }
    
    pub(crate) async fn initial_sync(&self) {
        let inner_dir = self.dir.inner.read().await;
        for node in inner_dir.neighbors.clone() {
            if node.1 {
                let node_id = NodeId::from_bytes(&node.0).unwrap();
                info!("Attempting to sync with {}", &node_id);
                match self.syncify_prot.connect(node_id).await {
                    Ok(mut conn) => {
                        let request = SyncifyPacket::Request {
                            head: *inner_dir.state.hash().as_bytes()
                        };
                        conn.send_packet(self.dir.clone(), request).await;
                    }
                    Err(err) => {
                        error!("{}", err);
                    }
                }
                break;
            }
        }
    }
}
