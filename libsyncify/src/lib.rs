use iroh::Endpoint;
use iroh::protocol::Router;
use iroh_blobs::{ALPN as BLOBS_ALPN, net_protocol::Blobs};
use iroh_docs::{ALPN as DOCS_ALPN, protocol::Docs};
use iroh_gossip::{ALPN as GOSSIP_ALPN, net::Gossip};

pub struct Syncify {
    router: Router,
}

impl Syncify {
    async fn new() -> anyhow::Result<Self> {
        let endpoint = Endpoint::builder()
            .discovery_n0()
            .discovery_local_network()
            .bind().await?;

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
        })
    }
}
