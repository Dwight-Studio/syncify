use crate::engine::actor::Event::Sync;
use crate::engine::actor::SyncEvent;
use crate::engine::serial_state::SerialDelta;
use crate::store::StoreManager;
use crate::SharedDirectory;
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, Error, Key, KeyInit, XChaCha20Poly1305, XNonce};
use futures_lite::future::Boxed;
use iroh::endpoint::{Connecting, Connection, ReadExactError, RecvStream, SendStream, VarInt};
use iroh::protocol::ProtocolHandler;
use iroh::{Endpoint, NodeAddr, NodeId};
use log::info;
use rkyv::rancor::Error as RancorError;
use rkyv::{Archive, Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

const HEADER_SIZE: usize = 48 + 1;

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

#[derive(Clone)]
pub struct SyncifyProtocol {
    pub(crate) store: Arc<RwLock<StoreManager>>,
    pub(crate) endpoint: Endpoint
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
            
            let mut conn = SyncifyConnection::accept_new(conn).await.unwrap();

            // Waiting for the Request packet
            let request = {
                match conn.receive_packet(&store).await {
                    Ok(packet) => { packet }
                    Err(err) => { conn.close(1, err); return Ok(()) }
                }
            };

            if let SyncifyPacket::Request { head } = request.1 {
                match store.read().await.get_shared_dir(&request.0) {
                    Some(dir) => match dir.inner.write().await.handle.clone() {
                        None => { conn.close(1, SyncifyProtocolError::ProcessingError) }
                        Some(handle) => {
                            info!("Sending event RequestDeltas");
                            handle.send(Sync(SyncEvent::RequestDeltas(conn))).await?;
                        }
                    },
                    None => { conn.close(1, SyncifyProtocolError::ProcessingError) }
                }
            }

            Ok(())
        })
    }
}

impl SyncifyProtocol {
    pub async fn connect(&self, node_id: NodeId) -> Result<SyncifyConnection, anyhow::Error> {
        Ok(SyncifyConnection::open_new(node_id, self.endpoint.clone()).await?)
    }
}

pub struct SyncifyConnection {
    connection: Connection,
    tx: SendStream,
    rx: RecvStream
}

impl SyncifyConnection {
    pub async fn open_new(node_id: NodeId, endpoint: Endpoint) -> Result<Self, anyhow::Error> {
        let connection = endpoint.connect(NodeAddr::new(node_id), SYNCIFY_ALPN).await?;
        let (tx, rx) = connection.open_bi().await?;

        info!("Opening request to {}", node_id);
        
        Ok(Self{connection, tx, rx})
    }

    pub async fn accept_new(connecting: Connecting) -> Result<Self, anyhow::Error> {
        let connection = connecting.await?;
        let (tx, rx) = connection.accept_bi().await?;
        info!("Incoming sync request from {}", connection.remote_node_id()?);

        Ok(Self{connection, tx, rx})
    }

    pub async fn send_packet(&mut self, dir: SharedDirectory, packet: SyncifyPacket) {
        let cipher = XChaCha20Poly1305::new(&Key::from(dir.verif_key.to_bytes()));
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        
        let packet_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&packet).unwrap();
        let crypted_bytes = cipher.encrypt(&nonce, &*packet_bytes).unwrap();

        let header = SyncifyPacket::Header {packet_size: crypted_bytes.len() as u64, nonce: <[u8; 24]>::try_from(nonce.as_slice()).unwrap(), uuid: dir.uuid};
        let header_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&header).unwrap();
        
        self.tx.write(header_bytes.as_slice()).await.unwrap();
        self.tx.write(crypted_bytes.as_slice()).await.unwrap();
        self.tx.stopped().await;
    }

    //noinspection RsTraitObligations
    pub async fn receive_packet(&mut self, store: &Arc<RwLock<StoreManager>>) -> Result<(Uuid, SyncifyPacket), SyncifyProtocolError> {
        let mut header_data = [0u8; HEADER_SIZE];
        self.rx.read_exact(&mut header_data).await.map_err(SyncifyProtocolError::ReadError)?;

        let deserialized_header = rkyv::from_bytes::<SyncifyPacket, RancorError>(&header_data).map_err(SyncifyProtocolError::DeserializeError)?;
        match deserialized_header {
            SyncifyPacket::Header {packet_size, nonce, uuid} => {
                if let Some(dir) = store.read().await.get_shared_dir(&uuid) {
                    let mut packet_buffer = vec![0u8; packet_size as usize];
                    self.rx.read_exact(&mut packet_buffer).await.map_err(SyncifyProtocolError::ReadError)?;

                    let cipher = XChaCha20Poly1305::new(&Key::from(dir.verif_key.to_bytes()));
                    let decrypted_bytes = cipher.decrypt(&XNonce::from(nonce), packet_buffer.as_ref()).map_err(SyncifyProtocolError::DecryptionError)?;
                    let deserialized_packet = rkyv::from_bytes::<SyncifyPacket, RancorError>(&decrypted_bytes).map_err(SyncifyProtocolError::DeserializeError)?;

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
    
    pub fn close(&self, err_code: u32, err: SyncifyProtocolError) {
        self.connection.close(VarInt::from_u32(err_code), err.to_string().as_bytes());
    }
}

impl Drop for SyncifyConnection {
    fn drop(&mut self) {
        if self.connection.close_reason().is_none() {
            self.connection.close(VarInt::from_u32(1), b"Connection dropped!");
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
    DecryptionError(Error),
    
    #[error("Unable to process data")]
    ProcessingError
}
