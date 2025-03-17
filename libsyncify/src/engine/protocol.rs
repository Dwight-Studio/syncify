use crate::engine::actor::Event::Sync;
use crate::engine::actor::SyncEvent;
use crate::engine::serial_state::SerialDelta;
use crate::store::StoreManager;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{Error, Key, KeyInit, XChaCha20Poly1305, XNonce};
use futures_lite::future::Boxed;
use iroh::endpoint::{Connecting, ReadExactError, RecvStream, VarInt};
use iroh::protocol::ProtocolHandler;
use log::{info, warn};
use rkyv::rancor::Error as RancorError;
use rkyv::{deserialize, Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

const HEADER_SIZE: usize = 48;

#[repr(u8)]
#[derive(Archive, Serialize, Deserialize)]
pub enum SyncifyPacket {
    Header {   // This packet has a fixed size of 48 bytes
        packet_size: u64,
        nonce: [u8; 24],
        uuid: Uuid
    } = 0,     // This number should NEVER change
    Request {
        head: [u8; 32]
    } = 1,     // This number should NEVER change
    Success {
        pool: HashMap<[u8; 32], SerialDelta>
    } = 2,     // This number should NEVER change
    Failed = 3 // This number should NEVER change
}

pub const SYNCIFY_ALPN: &[u8] = b"/syncify/1";

pub struct SyncifyProtocol {
    pub(super) store: Arc<RwLock<StoreManager>>,
}

impl Debug for SyncifyProtocol {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "COUCOU")
    }
}

impl ProtocolHandler for SyncifyProtocol {
    fn accept(&self, conn: Connecting) -> Boxed<anyhow::Result<()>> {
        let store = self.store.clone();
        Box::pin(async move {
            let connection = conn.await.unwrap();
            info!(
                "Incoming connection from {}",
                connection.remote_node_id().unwrap().to_string()
            );

            let (tx, mut rx) = connection.accept_bi().await.unwrap();
            
            // Waiting for the Request packet
            let request = {
                match Self::receive_packet(&mut rx, &store).await {
                    Ok(packet) => { packet }
                    Err(err) => { connection.close(VarInt::from_u32(1), err.to_string().as_bytes()); return Ok(()) }
                }
            };
            match request.1 {
                SyncifyPacket::Header { .. } => {}
                SyncifyPacket::Request { head } => {
                    match store.read().await.get_shared_dir(&request.0) {
                        Some(dir) => match dir.inner.write().await.handle.clone() {
                            None => { connection.close(VarInt::from_u32(1), b"Cannot get the dir handle") }
                            Some(handle) => {
                                if let Err(err) = handle.send(Sync(SyncEvent::RequestHashes(tx))).await {
                                    connection.close(VarInt::from_u32(1), err.to_string().as_bytes())
                                }
                            }
                        },
                        None => { connection.close(VarInt::from_u32(1), b"Cannot get the shared dir") }
                    }
                }
                SyncifyPacket::Success { .. } => {}
                SyncifyPacket::Failed => {}
            }

            Ok(())
        })
    }
}

impl SyncifyProtocol {
    async fn receive_packet(rx: &mut RecvStream, store: &Arc<RwLock<StoreManager>>) -> Result<(Uuid, SyncifyPacket), SyncifyProtocolError> {
        let mut header_data = [0u8; HEADER_SIZE];
        rx.read_exact(&mut header_data).await.map_err(SyncifyProtocolError::ReadError)?;
        
        let archived_header = rkyv::access::<ArchivedSyncifyPacket, RancorError>(&header_data).map_err(SyncifyProtocolError::DeserializeError)?;
        let deserialized_header = deserialize::<SyncifyPacket, RancorError>(archived_header).map_err(SyncifyProtocolError::DeserializeError)?;
        match deserialized_header {
            SyncifyPacket::Header {packet_size, nonce, uuid} => {
                if let Some(dir) = store.read().await.get_shared_dir(&uuid) {
                    let mut packet_buffer = vec![0u8; packet_size as usize];
                    rx.read_exact(&mut packet_buffer).await.map_err(SyncifyProtocolError::ReadError)?;

                    let cipher = XChaCha20Poly1305::new(&Key::from(dir.verif_key.to_bytes()));
                    let decrypted_bytes = cipher.decrypt(&XNonce::from(nonce), packet_buffer.as_ref()).map_err(SyncifyProtocolError::DecryptionError)?;
                    let archived_packet = rkyv::access::<ArchivedSyncifyPacket, RancorError>(&decrypted_bytes).map_err(SyncifyProtocolError::DeserializeError)?;
                    let deserialized_packet = deserialize::<SyncifyPacket, RancorError>(archived_packet).map_err(SyncifyProtocolError::DeserializeError)?;

                    Ok((uuid, deserialized_packet))
                } else {
                    Err(SyncifyProtocolError::UuidDoesNotExists)
                }
            }
            SyncifyPacket::Request { .. } => { Err(SyncifyProtocolError::HeaderMissing(String::from("Request"))) }
            SyncifyPacket::Success { .. } => { Err(SyncifyProtocolError::HeaderMissing(String::from("Success"))) }
            SyncifyPacket::Failed => { Err(SyncifyProtocolError::HeaderMissing(String::from("Failed"))) }
        }
    }
}

#[derive(Error, Debug)]
pub enum SyncifyProtocolError {
    #[error("Read error: {0}")]
    ReadError(ReadExactError),
    
    #[error("Unable to deserialize received data: {0}")]
    DeserializeError(RancorError),
    
    #[error("Missing header packet. Received {0}")]
    HeaderMissing(String),
    
    #[error("Uuid does not exists")]
    UuidDoesNotExists,
    
    #[error("Cannot decrypt the packet: {0}")]
    DecryptionError(Error)
}
