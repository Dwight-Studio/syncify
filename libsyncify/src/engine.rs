use std::io;
use iroh::Endpoint;
use iroh::protocol::Router;
use iroh_blobs::net_protocol::Blobs;
use iroh_docs::protocol::Docs;
use iroh_gossip::net::Gossip;
use iroh_gossip::rpc::proto::{Request, Response};
use quic_rpc::transport::flume::FlumeConnector;
use thiserror::Error;
use crate::get_app_dir;
use crate::config::SyncifyConfig;

const DOWNLOAD_DIRNAME: &str = "download";
const DATABASE_DIRNAME: &str = "database";

pub struct Engine {
    router: Router,
    blobs_client: iroh_blobs::rpc::client::blobs::MemClient,
    gossip_client: iroh_gossip::rpc::client::Client<FlumeConnector<Response, Request>>,
    docs_client: iroh_docs::rpc::client::docs::MemClient,
}

impl Engine {
    pub async fn new(config: &SyncifyConfig) -> Result<Self, EngineError> {
        let endpoint = Endpoint::builder()
            .secret_key(config.secret_key.clone())
            .alpns(vec![
                iroh_blobs::ALPN.to_vec(),
                iroh_gossip::ALPN.to_vec(),
                iroh_docs::ALPN.to_vec()
            ])
            .discovery_n0()
            .discovery_local_network()
            .user_data_for_discovery(config.user_data.clone())
            .bind()
            .await
            .map_err(|e| EngineError::Endpoint(e))?;

        // Router
        let builder = Router::builder(endpoint);

        // Blobs protocole
        let download_dir = get_app_dir().join(DOWNLOAD_DIRNAME);

        if !download_dir.exists() {
            tokio::fs::create_dir_all(&download_dir)
                .await
                .map_err(|e| EngineError::MakeDir(e))?
        }

        let blobs = Blobs::persistent(download_dir)
            .await
            .map_err(|e| EngineError::Blobs(e))?
            .build(builder.endpoint());
        let blobs_client = blobs.client().to_owned();

        // Gossip protocol
        let gossip = Gossip::builder()
            .spawn(builder.endpoint().clone())
            .await
            .map_err(|e| EngineError::Gossip(e))?;
        let gossip_client = gossip.client().to_owned();

        // Docs protocol
        let database_dir = get_app_dir().join(DATABASE_DIRNAME);

        if !database_dir.exists() {
            tokio::fs::create_dir_all(&database_dir)
                .await
                .map_err(|e| EngineError::MakeDir(e))?;
        }
        let docs = Docs::persistent(database_dir).spawn(&blobs, &gossip)
            .await
            .map_err(|e| EngineError::Docs(e))?;
        let docs_client = docs.client().to_owned();

        Ok(Self {
            router: builder
                .accept(iroh_blobs::ALPN.to_vec(), blobs)
                .accept(iroh_gossip::ALPN, gossip)
                .accept(iroh_docs::ALPN, docs)
                .spawn()
                .await
                .map_err(|e| EngineError::Router(e))?,
            blobs_client,
            gossip_client,
            docs_client
        })
    }

    pub async fn destroy(&self) {
        let _ = self.router.shutdown().await;
    }
}

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("Filesystem error: Cannot create directory ({0})")]
    MakeDir(io::Error),

    #[error("Endpoint error: {0}")]
    Endpoint(anyhow::Error),

    #[error("Blobs error: {0}")]
    Blobs(anyhow::Error),
    
    #[error("Gossip error: {0}")]
    Gossip(iroh_gossip::net::Error),

    #[error("Docs error: {0}")]
    Docs(anyhow::Error),

    #[error("Router error: {0}")]
    Router(anyhow::Error),
}