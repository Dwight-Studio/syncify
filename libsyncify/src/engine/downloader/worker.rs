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
use crate::engine::downloader::writer::{DownloadedChunk, FailedChunk, WriterChunk};
use crate::engine::job::RemoteProvision;
use crate::engine::protocol::{BlobsPacket, SyncifyPacket, SyncifyProtocol};
use async_channel::Receiver;
use blake3::Hash;
use iroh_base::NodeId;
use std::io::Read;
use tokio::sync::mpsc::Sender;

pub struct DownloadChunk {
    pub(crate) index: u64,
    pub(crate) hash: Hash,
    pub(crate) writer_tx: Sender<WriterChunk>,
    pub(crate) dir: SharedDirectory,
}

pub struct DownloadWorker {
    pub(crate) rx: Receiver<DownloadChunk>,
    pub(crate) proto: SyncifyProtocol,
}

impl DownloadWorker {
    pub async fn run(&mut self) {
        while let Ok(rcv) = self.rx.recv().await {
            if rcv.writer_tx.is_closed() {
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
                            let _ = rcv.writer_tx.send(WriterChunk::FailedChunk(FailedChunk {
                                index: rcv.index,
                            })).await;
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
                                            let _ = rcv.writer_tx.send(WriterChunk::FailedChunk(FailedChunk {
                                                index: rcv.index,
                                            })).await;
                                        } else {
                                            // If the writer channel is closed, it means that we cancelled
                                            // the download, don't do anything
                                            let _ = rcv
                                            .writer_tx
                                            .send(WriterChunk::DownloadedChunk(DownloadedChunk {
                                                index: rcv.index,
                                                data: decoded,
                                                node_id: *node.0,
                                            }))
                                            .await;
                                        }
                                    }
                                    _ => unreachable!(), // Will never happen
                                }
                            }
                            Err(_) => {
                                let _ = rcv.writer_tx.send(WriterChunk::FailedChunk(FailedChunk {
                                    index: rcv.index,
                                })).await;
                            }
                        }
                    }
                } else {
                    let _ = rcv.writer_tx.send(WriterChunk::FailedChunk(FailedChunk {
                        index: rcv.index,
                    })).await;
                }
            }
        }
    }
}
