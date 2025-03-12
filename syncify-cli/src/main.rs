mod logger;

use crate::logger::SyncifyLogger;
use iroh::{NodeAddr, PublicKey};
use libsyncify::Syncify;
use log::{debug, info};
use spdlog::Level;
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let logger = SyncifyLogger::new();
    logger.set_max_level(Level::Info);

    let mut syncify = Syncify::new().await.unwrap();

    match arg_refs.as_slice() {
        ["start"] => {
            syncify.start_sync().await.unwrap();
            let shared_dir = syncify.create_shared_directory(PathBuf::from("test")).await.unwrap();
            let ep = syncify.get_node_endpoint();

            info!("Node id: {}", ep.node_id());
            info!("UUID: {}", shared_dir.uuid());
            loop {}
        }
        ["connect", node_id, shared_dir_uuid] => {
            syncify.start_sync().await.unwrap();
            let ep = syncify.get_node_endpoint();

            info!("Connecting to {}", node_id);

            let conn = ep
                .connect(
                    NodeAddr::new(PublicKey::from_str(node_id).unwrap()),
                    b"/syncify/1",
                )
                .await
                .unwrap();
            info!("Connection established!");

            let (mut tx, mut rx) = conn.open_bi().await.unwrap();
            tx.write(b"").await.unwrap();

            tx.write(Uuid::from_str(shared_dir_uuid).unwrap().as_bytes()).await.unwrap();

            let mut recevice_buf = [0u8; 8];
            loop {
                if recevice_buf.as_slice() == b"RECEIVED" || recevice_buf.as_slice() == b"CANCELED" {
                    break;
                }
                rx.read_exact(&mut recevice_buf).await.unwrap();
                info!("Read {}", String::from_utf8_lossy(&recevice_buf));
            }
            //tx.stopped().await.unwrap();
        }
        ["receive", ticket, file] => debug!("RECEIVE from {} {}", ticket, file),
        _ => debug!("WTF"),
    }

    syncify.stop_sync().await;
}
