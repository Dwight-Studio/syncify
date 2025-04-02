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
use iroh::endpoint::{ClosedStream, Connection, ReadError, ReadToEndError, RecvStream, SendStream, VarInt, WriteError};
use iroh::protocol::ProtocolHandler;
use iroh::{Endpoint, NodeAddr, NodeId};
use log::{debug};
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

/// The [`SyncifyProtocol`] struct, used to store the active connections.
pub struct SyncifyProtocol {
    connections: Vec<SyncifyConnection>,
}

impl SyncifyProtocol {
    /// Use or open a [`Connection`] and open a new bidirectional stream on it.
    pub async fn open_stream(&mut self, dir: &SharedDirectory, node_id: NodeId) -> Result<SyncifyStream, SyncifyProtocolError> {
        todo!()
    }
}

/// The [`SyncifyStream`] struct, used to send and receive data in a stream.
pub struct SyncifyStream {
    dir: SharedDirectory,
    send_stream: SendStream,
    recv_stream: RecvStream,
}

impl SyncifyStream {
    /// Send a packet.
    pub async fn send(&mut self, packet: &SyncifyPacket) -> Result<(), SyncifyProtocolError> {
        let cipher = XChaCha20Poly1305::new(&Key::from(self.dir.read_key.to_bytes()));
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

        let packet_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(packet).unwrap();
        let cipher_bytes = cipher.encrypt(&nonce, &*packet_bytes).unwrap();

        let header = HeaderPacket {
            packet_size: cipher_bytes.len() as u64,
            nonce: <[u8; 24]>::try_from(nonce.as_slice()).unwrap(),
            uuid: self.dir.uuid,
        };
        let header_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&header).unwrap();

        self.send_stream.write_all(header_bytes.as_slice()).await.unwrap();
        self.send_stream.write_all(cipher_bytes.as_slice()).await.unwrap();

        Ok(())
    }

    /// Send a packet.
    pub async fn recv(&mut self) -> Result<SyncifyPacket, SyncifyProtocolError> {
        let header = self.recv_header().await?;
        let packet = self.recv_packet(&header).await?;

        Ok(packet)
    }

    //noinspection RsTraitObligations
    /// Receive a [`HeaderPacket`].
    async fn recv_header(&mut self) -> Result<HeaderPacket, SyncifyProtocolError> {
        let mut header_data = [0u8; HEADER_SIZE];
        self.recv_stream.read(&mut header_data)
            .await
            .map_err(|e| SyncifyProtocolError::ReadError(e, String::from("header")))?;

        let res = rkyv::from_bytes::<HeaderPacket, RancorError>(&header_data)
            .map_err(SyncifyProtocolError::DeserializeError)?;

        if self.dir.uuid != res.uuid {
            return Err(SyncifyProtocolError::WrongRecipient(res.uuid));
        }

        Ok(res)
    }

    //noinspection RsTraitObligations
    /// Receive a [`SyncifyPacket`].
    async fn recv_packet(
        &mut self,
        header_packet: &HeaderPacket
    ) -> Result<SyncifyPacket, SyncifyProtocolError> {
        let packet_buffer;
        match self.recv_stream.read_to_end(header_packet.packet_size as usize).await {
            Ok(vec) => {packet_buffer = vec}
            Err(err) => {return Err(SyncifyProtocolError::ReadToEndError(err, String::from("syncify-packet")))}
        }

        let cipher = XChaCha20Poly1305::new(&Key::from(self.dir.read_key.to_bytes()));
        let decrypted_bytes = cipher
            .decrypt(&XNonce::from(header_packet.nonce), packet_buffer.as_ref())
            .map_err(SyncifyProtocolError::DecryptionError)?;

        rkyv::from_bytes::<SyncifyPacket, RancorError>(&decrypted_bytes).map_err(SyncifyProtocolError::DeserializeError)
    }

    /// Close the stream.
    pub fn close(mut self) -> Result<(), SyncifyProtocolError> {
        self.send_stream.finish().map_err(SyncifyProtocolError::ClosedStream)
    }
}


/// The [`SyncifyProtocolHandler`] struct, used to handle connections using this protocol.
pub struct SyncifyProtocolHandler {
    protocol: Arc<RwLock<SyncifyProtocol>>,
    store: Arc<RwLock<StoreManager>>,
    downloader: DownloaderHandle,
}

impl SyncifyProtocolHandler {
    pub fn new(protocol: Arc<RwLock<SyncifyProtocol>>, store: Arc<RwLock<StoreManager>>, downloader: DownloaderHandle) -> Self {
        Self { protocol, store, downloader }
    }
}

impl Debug for SyncifyProtocolHandler {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "COUCOU")
    }
}

impl ProtocolHandler for SyncifyProtocolHandler {
    //noinspection RsTraitObligations
    /// Manages incoming SyncifyProtocol connections
    fn accept(&self, connection: Connection) -> Boxed<anyhow::Result<()>> {
        let protocol = self.protocol.clone();
        let store = self.store.clone();
        let downloader = self.downloader.clone();
        Box::pin(async move {
            let (_tx, mut rx) = connection.accept_bi().await.unwrap();

            let mut header_buffer = [0u8; HEADER_SIZE];
            rx.read_exact(&mut header_buffer).await.unwrap();

            let header = rkyv::from_bytes::<HeaderPacket, RancorError>(&header_buffer)
                .map_err(SyncifyProtocolError::DeserializeError)?;

            let conn = SyncifyConnection::accept_new(connection).await.unwrap();

            let dir = {
                match store.read().await.get_shared_dir(&header.uuid) {
                    None => {
                        conn.close(1, SyncifyProtocolError::UuidDoesNotExists);
                        return Ok(());
                    }
                    Some(dir) => dir,
                }
            };
            let mut packet_buffer = vec![0u8; header.packet_size as usize];
            rx.read(&mut packet_buffer)
                .await
                .map_err(|e| SyncifyProtocolError::ReadError(e, String::from("syncify_packet")))?;

            let cipher = XChaCha20Poly1305::new(&Key::from(dir.read_key.to_bytes()));
            let decrypted_bytes = cipher
                .decrypt(&XNonce::from(header.nonce), packet_buffer.as_ref())
                .map_err(SyncifyProtocolError::DecryptionError)?;

            let packet = rkyv::from_bytes::<SyncifyPacket, RancorError>(&decrypted_bytes)
                .map_err(SyncifyProtocolError::DeserializeError)?;

            match packet {
                SyncifyPacket::Sync(sync_packet) => {
                    if let SyncPacket::Request { head } = sync_packet {
                        dir.handle()
                            .await
                            .clone()
                            .send(Sync(SyncEvent::RequestSync(conn.clone(), blake3::Hash::from(head))))
                            .await;
                    }
                }
                SyncifyPacket::Blobs(blobs_packet) => {
                    if let BlobsPacket::BlobRequest { file_hash, chunk_index } = blobs_packet {
                        downloader
                            .send(DownloaderEvent::Supply {
                                conn: conn.clone(),
                                uuid: dir.uuid,
                                file_hash: Hash::from(file_hash),
                                chunk_index,
                            })
                            .await;
                    }
                }
            }

            Ok(())
        })
    }
}

#[derive(Clone)]
/// Manages SyncifyProtocol connections
pub struct SyncifyConnection {
    connection: Connection,
}

impl SyncifyConnection {
    pub async fn connect(node_id: NodeId, endpoint: Endpoint) -> Result<Self, anyhow::Error> {
        let connection = endpoint.connect(NodeAddr::new(node_id), SYNCIFY_ALPN).await?;

        Ok(Self { connection })
    }

    async fn accept_new(connection: Connection) -> Result<Self, anyhow::Error> {
        Ok(Self { connection })
    }

    pub fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }

    /// Close the connection
    pub fn close(&self, err_code: u32, err: SyncifyProtocolError) {
        debug!("CLOSED: {}", err);
        self.connection
            .close(VarInt::from_u32(err_code), err.to_string().as_bytes());
    }

    pub fn remote(&self) -> NodeId {
        self.connection.remote_node_id().unwrap()
    }
}

#[derive(Error, Debug)]
pub enum SyncifyProtocolError {
    #[error("Read error: {0}, {1}")]
    ReadError(ReadError, String),

    #[error("Read error: {0}, {1}")]
    ReadToEndError(ReadToEndError, String),

    #[error("Write error: {0}")]
    WriteError(WriteError),

    #[error("Closed stream: {0}")]
    ClosedStream(ClosedStream),

    #[error("Unable to deserialize received data: {0}")]
    DeserializeError(RancorError),

    #[error("Missing header packet. Received {0}")]
    HeaderMissing(String),

    #[error("Uuid does not exists")]
    UuidDoesNotExists,

    #[error("Cannot decrypt the packet: {0}")]
    DecryptionError(Error),

    #[error("Wrong recipient: {0}")]
    WrongRecipient(Uuid),
}
