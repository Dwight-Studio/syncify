use crate::config::SyncifyConfig;
use iroh::protocol::Router;
use iroh::Endpoint;
use iroh_blobs::{net_protocol::Blobs, ALPN as BLOBS_ALPN};
use iroh_docs::{protocol::Docs, ALPN as DOCS_ALPN};
use iroh_gossip::{net::Gossip, ALPN as GOSSIP_ALPN};

mod config;

pub struct Syncify {
    router: Router,
    config: SyncifyConfig,
}

impl Syncify {
    pub async fn new() -> anyhow::Result<Self> {
        let config = SyncifyConfig::new()?;

        println!("{}", config);

        let endpoint = Endpoint::builder()
            .secret_key(config.secret_key)
            .alpns(vec![
                BLOBS_ALPN.to_vec(),
                GOSSIP_ALPN.to_vec(),
                DOCS_ALPN.to_vec(),
            ])
            .discovery_n0()
            .discovery_local_network()
            .user_data_for_discovery(config.user_data)
            .bind()
            .await?;

        // create a router builder, we will add the
        // protocols to this builder and then spawn
        // the router
        let builder = Router::builder(endpoint);

        // build the blobs protocol
        let blobs = Blobs::memory().build(builder.endpoint());

        // build the gossip protocol
        let gossip = Gossip::builder().spawn(builder.endpoint().clone()).await?;

        // build the docs protocol
        let docs = Docs::memory().spawn(&blobs, &gossip).await?;

        Ok(Self {
            router: builder
                .accept(BLOBS_ALPN, blobs)
                .accept(GOSSIP_ALPN, gossip)
                .accept(DOCS_ALPN, docs)
                .spawn()
                .await?,
            config: SyncifyConfig::new()?
        })
    }
}
