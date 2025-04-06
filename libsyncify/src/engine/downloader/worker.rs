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
use crate::SharedDirectory;
use crate::engine::downloader::CHUNK_SIZE;
use crate::engine::downloader::writer::DownloadedChunk;
use crate::engine::job::RemoteProvision;
use crate::engine::protocol::{BlobsPacket, SyncifyPacket, SyncifyProtocol};
use async_channel::Receiver;
use blake3::Hash;
use iroh_base::NodeId;
use log::debug;
use std::io::Read;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Sender;

pub struct DownloadChunk {
    pub(crate) index: u64,
    pub(crate) hash: Hash,
    pub(crate) writer_tx: Sender<DownloadedChunk>,
    pub(crate) dir: SharedDirectory,
}

pub struct DownloadWorker {
    pub(crate) rx: Receiver<DownloadChunk>,
    pub(crate) cancel_list: Arc<RwLock<Vec<Hash>>>,
    pub(crate) proto: SyncifyProtocol,
}

impl DownloadWorker {
    pub async fn run(&mut self) {
        while let Ok(rcv) = self.rx.recv().await {
            if self.cancel_list.read().await.contains(&rcv.hash) {
                continue;
            }
            if let Some(node_list) = rcv.dir.remote_provisions.read().await.get(&rcv.hash) {
                let nodes: Vec<(&NodeId, &RemoteProvision)> = node_list.iter().collect();
                let node = nodes.get(rcv.index as usize % nodes.len()).unwrap();
                if !node.1.is_expired() {
                    if let Ok(mut connection) = self.proto.open_stream(&rcv.dir, *node.0).await {
                        let packet = SyncifyPacket::Blob(BlobsPacket::BlobRequest {
                            file_hash: rcv.hash,
                            chunk_index: rcv.index,
                        });
                        if connection.send(&packet).await.is_err() {
                            debug!("Not implemented: failed to send BlobRequest");
                            // TODO: Requeue the chunk because we failed to send the BlobRequest packet
                            continue;
                        }
                        match connection.recv().await {
                            Ok(packet) => {
                                match packet {
                                    SyncifyPacket::Blob(BlobsPacket::Blob { chunk }) => {
                                        let mut decoded = Vec::new();
                                        let mut decoder = bao::decode::SliceDecoder::new(
                                            &*chunk,
                                            &rcv.hash,
                                            CHUNK_SIZE as u64 * rcv.index,
                                            CHUNK_SIZE as u64,
                                        );

                                        if decoder.read_to_end(&mut decoded).is_err() {
                                            debug!("Not implemented: cannot decode");
                                            // TODO: Requeue the chunk because we failed to decode
                                        } else if let Err(err) = rcv
                                            .writer_tx
                                            .send(DownloadedChunk {
                                                index: rcv.index,
                                                data: decoded,
                                                node_id: *node.0,
                                            })
                                            .await
                                        {
                                            debug!("Not implemented: cannot send message to the writer ({err})");
                                            // TODO: Requeue the chunk
                                        }
                                    }
                                    _ => unreachable!(), // Will never happen
                                }
                            }
                            Err(err) => {
                                debug!("Not implemented: failed to receive packet ({err})");
                                // TODO: Requeue the chunk because we failed to receive the Blob packet
                            }
                        }
                    }
                } else {
                    debug!("Not implemented: remote provision expired");
                    // TODO: RemoteProvision expired, requeue the chunk, check if another Node have
                    //  the file and it is not expired, else broadcast provision request
                }
            }
        }
    }
}
