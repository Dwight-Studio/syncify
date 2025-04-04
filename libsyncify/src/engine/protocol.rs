/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
pub mod fsm;
pub mod incoming_sync;
pub mod outgoing_sync;

use crate::SharedDirectory;
use crate::engine::downloader::{DownloaderEvent, DownloaderHandle};
use crate::engine::manager::ManagerEvent::Sync;
use crate::engine::manager::SyncEvent;
use crate::engine::state::State;
use crate::store::StoreManager;
use blake3::Hash;
use chacha20poly1305::aead::{Aead, OsRng};
use chacha20poly1305::{AeadCore, Error, Key, KeyInit, XChaCha20Poly1305, XNonce};
use futures_lite::future::Boxed;
use iroh::endpoint::{
    ClosedStream, Connection, ReadExactError, RecvStream, SendStream, StoppedError, VarInt, WriteError,
};
use iroh::protocol::ProtocolHandler;
use iroh::{Endpoint, NodeId};
use iroh_base::NodeAddr;
use log::{debug, error};
use rkyv::rancor::Error as RancorError;
use rkyv::{Archive, Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

/// The size in bytes of the SyncifyPacket::Header packet variant
pub const HEADER_SIZE: usize = 48;

/// The ALPN that is used to open and accept connection on the SyncifyProtocol
pub const SYNCIFY_ALPN: &[u8] = b"/syncify/1";

#[derive(Archive, Serialize, Deserialize, Debug)]
/// This structure can be Serialized and Deserialized with rkyv and contains necessary elements
/// to deserialize the next SyncifyPacket from
pub(crate) struct HeaderPacket {
    packet_size: u64, // 8 bytes
    nonce: [u8; 24],  // 24 bytes
    uuid: Uuid,       // 16 bytes
}

#[repr(u8)]
#[derive(Archive, Serialize, Deserialize, Debug)]
/// Enumeration representing the data that can be transferred using SyncifyConnection
pub enum SyncifyPacket {
    Sync(SyncPacket),
    Blobs(BlobsPacket),
}

#[repr(u8)]
#[derive(Archive, Serialize, Deserialize, Debug)]
pub enum SyncPacket {
    Request { head: [u8; 32] } = 0,
    Success { state: State } = 1,
    Failed = 2,
}

#[repr(u8)]
#[derive(Archive, Serialize, Deserialize, Debug)]
pub enum BlobsPacket {
    BlobRequest { file_hash: [u8; 32], chunk_index: u64 } = 3,
    Blob { chunk: Vec<u8> } = 4,
}

#[derive(Clone)]
/// The [`SyncifyProtocol`] struct, used to store the active connections.
pub struct SyncifyProtocol {
    pub(crate) connections: Arc<RwLock<Vec<Connection>>>,
    pub(crate) ep: Endpoint,
    pub(crate) downloader: Option<DownloaderHandle>,
    pub(crate) store: Arc<RwLock<StoreManager>>,
}

impl SyncifyProtocol {
    pub fn new(ep: Endpoint, store: Arc<RwLock<StoreManager>>) -> Self {
        Self {
            connections: Arc::new(RwLock::new(Vec::new())),
            ep,
            downloader: None,
            store,
        }
    }

    pub fn set_downloader(&mut self, downloader: DownloaderHandle) {
        self.downloader = Some(downloader);
    }

    /// Use or open a [`Connection`] and open a new bidirectional stream on it.
    pub async fn open_stream(
        &mut self,
        dir: &SharedDirectory,
        node_id: NodeId,
    ) -> Result<SyncifyStream, SyncifyProtocolError> {
        let mut existing_conn: Option<&Connection> = None;
        let connections = self.connections.read().await;
        for connection in &*connections {
            if connection.remote_node_id().unwrap() == node_id {
                existing_conn = Some(connection);
                break;
            }
        }

        match existing_conn {
            Some(conn) => {
                let (tx, rx) = conn
                    .open_bi()
                    .await
                    .map_err(|e| SyncifyProtocolError::ConnectionError(e.to_string()))?;
                Ok(SyncifyStream::new(dir.clone(), tx, rx))
            }
            None => {
                let conn = self
                    .ep
                    .connect(NodeAddr::new(node_id), SYNCIFY_ALPN)
                    .await
                    .map_err(|e| SyncifyProtocolError::ConnectionError(e.to_string()))?;
                self.connections.write().await.push(conn.clone());
                let (tx, rx) = conn
                    .open_bi()
                    .await
                    .map_err(|e| SyncifyProtocolError::ConnectionError(e.to_string()))?;
                if let Some(downloader) = &self.downloader {
                    tokio::spawn(accept_connection(
                        conn,
                        self.connections.clone(),
                        self.store.clone(),
                        downloader.clone(),
                    ));
                } else {
                    error!("Downloader is not available")
                }
                Ok(SyncifyStream::new(dir.clone(), tx, rx))
            }
        }
    }
}

/// The [`SyncifyStream`] struct, used to send and receive data in a stream.
pub struct SyncifyStream {
    dir: SharedDirectory,
    send_stream: SendStream,
    recv_stream: RecvStream,
    cipher: XChaCha20Poly1305,
}

impl SyncifyStream {
    pub fn new(dir: SharedDirectory, send_stream: SendStream, recv_stream: RecvStream) -> Self {
        let cipher = XChaCha20Poly1305::new(&Key::from(dir.read_key.to_bytes()));
        Self {
            dir,
            send_stream,
            recv_stream,
            cipher,
        }
    }

    /// Generate non null nonce.
    fn generate_nonce() -> XNonce {
        loop {
            let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
            if <[u8; 24]>::from(nonce) != [0u8; 24] {
                return nonce;
            }
        }
    }

    /// Send an encrypted packet.
    pub async fn send(&mut self, packet: &SyncifyPacket) -> Result<(), SyncifyProtocolError> {
        let nonce = Self::generate_nonce();

        let packet_bytes =
            rkyv::to_bytes::<rkyv::rancor::Error>(packet).map_err(SyncifyProtocolError::Serialization)?;
        let cipher_bytes = self
            .cipher
            .encrypt(&nonce, &*packet_bytes)
            .map_err(SyncifyProtocolError::Encryption)?;

        let header = HeaderPacket {
            packet_size: cipher_bytes.len() as u64,
            nonce: <[u8; 24]>::from(nonce),
            uuid: self.dir.uuid,
        };
        let header_bytes =
            rkyv::to_bytes::<rkyv::rancor::Error>(&header).map_err(SyncifyProtocolError::Serialization)?;

        self.send_stream
            .write_all(header_bytes.as_slice())
            .await
            .map_err(SyncifyProtocolError::WriteError)?;
        self.send_stream
            .write_all(cipher_bytes.as_slice())
            .await
            .map_err(SyncifyProtocolError::WriteError)?;

        Ok(())
    }

    /// Send a plain packet.
    pub async fn send_plain(&mut self, packet: &SyncifyPacket) -> Result<(), SyncifyProtocolError> {
        let packet_bytes =
            rkyv::to_bytes::<rkyv::rancor::Error>(packet).map_err(SyncifyProtocolError::Serialization)?;

        let header = HeaderPacket {
            packet_size: packet_bytes.len() as u64,
            nonce: [0u8; 24],
            uuid: self.dir.uuid,
        };
        let header_bytes =
            rkyv::to_bytes::<rkyv::rancor::Error>(&header).map_err(SyncifyProtocolError::Serialization)?;

        self.send_stream
            .write_all(header_bytes.as_slice())
            .await
            .map_err(SyncifyProtocolError::WriteError)?;
        self.send_stream
            .write_all(packet_bytes.as_slice())
            .await
            .map_err(SyncifyProtocolError::WriteError)?;

        Ok(())
    }

    /// Send a packet.
    pub async fn recv(&mut self) -> Result<SyncifyPacket, SyncifyProtocolError> {
        let header = self.recv_header().await?;
        let packet = if header.nonce == [0u8; 24] {
            self.recv_plain_packet(&header).await?
        } else {
            self.recv_packet(&header).await?
        };

        Ok(packet)
    }

    //noinspection RsTraitObligations
    /// Receive a [`HeaderPacket`].
    async fn recv_header(&mut self) -> Result<HeaderPacket, SyncifyProtocolError> {
        let mut header_data = [0u8; HEADER_SIZE];
        self.recv_stream
            .read_exact(&mut header_data)
            .await
            .map_err(|e| SyncifyProtocolError::ReadExactError(e, String::from("header")))?;

        let res =
            rkyv::from_bytes::<HeaderPacket, RancorError>(&header_data).map_err(SyncifyProtocolError::Deserialize)?;

        if self.dir.uuid != res.uuid {
            return Err(SyncifyProtocolError::WrongRecipient(res.uuid));
        }

        Ok(res)
    }

    //noinspection RsTraitObligations
    /// Receive a [`SyncifyPacket`].
    async fn recv_packet(&mut self, header_packet: &HeaderPacket) -> Result<SyncifyPacket, SyncifyProtocolError> {
        let mut packet_buffer = vec![0u8; header_packet.packet_size as usize];
        self.recv_stream
            .read_exact(&mut packet_buffer)
            .await
            .map_err(|e| SyncifyProtocolError::ReadExactError(e, String::from("syncify_packet")))?;

        let decrypted_bytes = self
            .cipher
            .decrypt(&XNonce::from(header_packet.nonce), packet_buffer.as_ref())
            .map_err(SyncifyProtocolError::Decryption)?;

        rkyv::from_bytes::<SyncifyPacket, RancorError>(&decrypted_bytes).map_err(SyncifyProtocolError::Deserialize)
    }

    //noinspection RsTraitObligations
    /// Receive a [`SyncifyPacket`].
    async fn recv_plain_packet(&mut self, header_packet: &HeaderPacket) -> Result<SyncifyPacket, SyncifyProtocolError> {
        let mut packet_buffer = vec![0u8; header_packet.packet_size as usize];
        self.recv_stream
            .read_exact(&mut packet_buffer)
            .await
            .map_err(|e| SyncifyProtocolError::ReadExactError(e, String::from("syncify_packet")))?;

        rkyv::from_bytes::<SyncifyPacket, RancorError>(&packet_buffer).map_err(SyncifyProtocolError::Deserialize)
    }

    /// Close the stream.
    pub async fn close(&mut self) -> Result<(), SyncifyProtocolError> {
        self.send_stream.finish().map_err(SyncifyProtocolError::ClosedStream)?;
        self.send_stream
            .stopped()
            .await
            .map_err(SyncifyProtocolError::StoppedStream)?;
        Ok(())
    }
}

#[derive(Clone)]
/// The [`SyncifyProtocolHandler`] struct, used to handle connections using this protocol.
pub struct SyncifyProtocolHandler {
    proto: SyncifyProtocol,
    store: Arc<RwLock<StoreManager>>,
    downloader: DownloaderHandle,
}

impl SyncifyProtocolHandler {
    pub fn new(proto: SyncifyProtocol, store: Arc<RwLock<StoreManager>>, downloader: DownloaderHandle) -> Self {
        Self {
            proto,
            store,
            downloader,
        }
    }
}

impl Debug for SyncifyProtocolHandler {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "COUCOU")
    }
}

impl ProtocolHandler for SyncifyProtocolHandler {
    /// Manages incoming SyncifyProtocol connections
    fn accept(&self, connection: Connection) -> Boxed<anyhow::Result<()>> {
        let proto = self.proto.clone();
        let store = self.store.clone();
        let downloader = self.downloader.clone();
        Box::pin(async move { accept_connection(connection, proto.connections.clone(), store, downloader).await })
    }
}

//noinspection RsTraitObligations
async fn accept_connection(
    connection: Connection,
    connections: Arc<RwLock<Vec<Connection>>>,
    store: Arc<RwLock<StoreManager>>,
    downloader: DownloaderHandle,
) -> anyhow::Result<()> {
    connections.write().await.push(connection.clone());

    while let Ok((tx, mut rx)) = connection.accept_bi().await {
        let mut header_buffer = [0u8; HEADER_SIZE];
        rx.read_exact(&mut header_buffer).await?;

        let header =
            rkyv::from_bytes::<HeaderPacket, RancorError>(&header_buffer).map_err(SyncifyProtocolError::Deserialize)?;

        let dir = {
            match store.read().await.get_shared_dir(&header.uuid) {
                None => {
                    connection.close(
                        VarInt::from_u32(1),
                        SyncifyProtocolError::UuidDoesNotExists.to_string().as_bytes(),
                    );
                    return Ok(());
                }
                Some(dir) => dir,
            }
        };

        let mut packet_buffer = vec![0u8; header.packet_size as usize];
        rx.read_exact(&mut packet_buffer)
            .await
            .map_err(|e| SyncifyProtocolError::ReadExactError(e, String::from("syncify_packet")))?;

        let cipher = XChaCha20Poly1305::new(&Key::from(dir.read_key.to_bytes()));
        let decrypted_bytes = cipher
            .decrypt(&XNonce::from(header.nonce), packet_buffer.as_ref())
            .map_err(SyncifyProtocolError::Decryption)?;

        let packet = rkyv::from_bytes::<SyncifyPacket, RancorError>(&decrypted_bytes)
            .map_err(SyncifyProtocolError::Deserialize)?;

        let stream = SyncifyStream {
            dir: dir.clone(),
            send_stream: tx,
            recv_stream: rx,
            cipher: XChaCha20Poly1305::new(&Key::from(dir.read_key.to_bytes())),
        };

        match packet {
            SyncifyPacket::Sync(sync_packet) => {
                if let SyncPacket::Request { head } = sync_packet {
                    dir.handle()
                        .await
                        .clone()
                        .send(Sync(SyncEvent::RequestSync(stream, blake3::Hash::from(head))))
                        .await;
                }
            }
            SyncifyPacket::Blobs(blobs_packet) => {
                if let BlobsPacket::BlobRequest { file_hash, chunk_index } = blobs_packet {
                    downloader
                        .send(DownloaderEvent::Supply {
                            conn: stream,
                            file_hash: Hash::from(file_hash),
                            chunk_index,
                        })
                        .await;
                }
            }
        }
    }

    debug!("Dropping connection with {}", connection.remote_node_id()?);

    connections.write().await.retain(|c| c.close_reason().is_none());

    Ok(())
}

#[derive(Error, Debug)]
pub enum SyncifyProtocolError {
    #[error("Read error: {0}, {1}")]
    ReadExactError(ReadExactError, String),

    #[error("Write error: {0}")]
    WriteError(WriteError),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Closed stream: {0}")]
    ClosedStream(ClosedStream),

    #[error("Stopped stream: {0}")]
    StoppedStream(StoppedError),

    #[error("Unable to deserialize received data: {0}")]
    Serialization(RancorError),

    #[error("Unable to deserialize received data: {0}")]
    Deserialize(RancorError),

    #[error("Uuid does not exists")]
    UuidDoesNotExists,

    #[error("Cannot decrypt the packet: {0}")]
    Encryption(Error),

    #[error("Cannot decrypt the packet: {0}")]
    Decryption(Error),

    #[error("Wrong recipient: {0}")]
    WrongRecipient(Uuid),
}
