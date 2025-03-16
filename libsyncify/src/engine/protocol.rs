use crate::store::StoreManager;
use futures_lite::future::Boxed;
use iroh::endpoint::Connecting;
use iroh::protocol::ProtocolHandler;
use log::debug;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

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
            debug!(
                "Incoming connection from {}",
                connection.remote_node_id().unwrap().to_string()
            );

            let (mut tx, mut rx) = connection.accept_bi().await.unwrap();
            let mut rcv = [0u8; 16];

            rx.read_exact(&mut rcv).await.unwrap();
            if store
                .read()
                .await
                .get_shared_dir(&Uuid::from_bytes(rcv))
                .is_some()
            {
                tx.write(b"RECEIVED").await.unwrap();
            } else {
                tx.write(b"CANCELED").await.unwrap();
            }

            connection.closed().await;

            Ok(())
        })
    }
}
