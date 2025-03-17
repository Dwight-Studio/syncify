use crate::engine::actor::SyncEvent;
use crate::engine::protocol::SyncifyPacket;
use crate::SharedDirectory;
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, Key, KeyInit, XChaCha20Poly1305};
use iroh_gossip::net::GossipSender;
use rkyv::rancor::Error;

pub(crate) struct SyncManager {
    topic: GossipSender,
    dir: SharedDirectory,
}

impl SyncManager {
    pub(crate) fn new(topic: GossipSender, dir: SharedDirectory) -> Self {
        Self { topic, dir }
    }

    pub(crate) async fn handle_events(&mut self, sync_event: SyncEvent) {
        match sync_event { 
            SyncEvent::RequestHashes(mut tx) => {
                let cipher = XChaCha20Poly1305::new(&Key::from(self.dir.verif_key.to_bytes()));
                let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

                let packet = SyncifyPacket::Failed;
                let packet_bytes = rkyv::to_bytes::<Error>(&packet).unwrap();
                let crypted_bytes = cipher.encrypt(&nonce, &*packet_bytes).unwrap();
                
                let header = SyncifyPacket::Header {packet_size: packet_bytes.len() as u64, nonce: <[u8; 24]>::try_from(nonce.as_slice()).unwrap(), uuid: self.dir.uuid};
                let header_bytes = rkyv::to_bytes::<Error>(&header).unwrap();
                
                tx.write(header_bytes.as_slice()).await.unwrap();
                tx.write(crypted_bytes.as_slice()).await.unwrap();
            }
        }
    }
}
