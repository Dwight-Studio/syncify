use std::fmt::{Debug, Formatter};
use futures_lite::future::Boxed;
use iroh::endpoint::{Connecting};
use iroh::protocol::ProtocolHandler;
use log::{debug};

pub const SYNCIFY_ALPN: &[u8] = b"/syncify/1";

pub struct SyncifyProtocol;

impl Debug for SyncifyProtocol {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "COUCOU")
    }
}

impl ProtocolHandler for SyncifyProtocol {
    fn accept(&self, conn: Connecting) -> Boxed<anyhow::Result<()>> {
        Box::pin(async move {
            let connection = conn.await.unwrap();
            debug!("Incoming connection from {}", connection.remote_node_id().unwrap().to_string());

            let (mut tx, mut rx) = connection.accept_bi().await.unwrap();
            let mut rcv = [0u8; 8];

            rx.read_exact(&mut rcv).await.unwrap();
            tx.write(b"RECEIVED").await.unwrap();

            connection.closed().await;

            Ok(())
        })
    }
}