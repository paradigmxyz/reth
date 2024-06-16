use discv5::{enr::secp256k1::rand, ListenConfig};
use futures_util::StreamExt;
use reth::{
    network::{
        config::SecretKey, NetworkConfig, NetworkEvent, NetworkEvents, NetworkHandle,
        NetworkManager,
    },
    primitives::Genesis,
    providers::test_utils::NoopProvider,
};
use reth_chainspec::ChainSpec;
use reth_tracing::tracing::info;
use std::{
    future::Future,
    net::{SocketAddrV4, SocketAddrV6},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

pub(crate) mod cli_ext;

pub(crate) struct Discv5Network {
    handle: NetworkHandle,
}

impl Discv5Network {
    pub async fn new(ipv4_addr: SocketAddrV4, ipv6_addr: SocketAddrV6) -> Self {
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let discv5_config = discv5::ConfigBuilder::new(ListenConfig::from_two_sockets(
            Some(ipv4_addr),
            Some(ipv6_addr),
        ))
        .build();

        let net_cfg = NetworkConfig::builder(secret_key)
            .chain_spec(custom_chain())
            .disable_discv4_discovery()
            .map_discv5_config_builder(|builder| builder.discv5_config(discv5_config))
            .build(NoopProvider::default());

        let network = NetworkManager::new(net_cfg).await.unwrap();

        let handle = network.handle().clone();
        tokio::spawn(network);

        Self { handle }
    }
}

impl Future for Discv5Network {
    type Output = eyre::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.as_mut();
        while let Poll::Ready(event) = this.handle.event_listener().poll_next_unpin(cx) {
            match event {
                Some(evt) => {
                    if let NetworkEvent::SessionEstablished { status, client_version, .. } = evt {
                        let chain = status.chain;
                        info!(?chain, ?client_version, "Session established with a new peer.");
                    }
                }
                None => return Poll::Ready(Ok(())),
            }
        }
        Poll::Pending
    }
}

fn custom_chain() -> Arc<ChainSpec> {
    let custom_genesis = r#"
{

    "nonce": "0x42",
    "timestamp": "0x0",
    "extraData": "0x5343",
    "gasLimit": "0x1388",
    "difficulty": "0x400000000",
    "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "coinbase": "0x0000000000000000000000000000000000000000",
    "alloc": {
        "0x6Be02d1d3665660d22FF9624b7BE0551ee1Ac91b": {
            "balance": "0x4a47e3c12448f4ad000000"
        }
    },
    "number": "0x0",
    "gasUsed": "0x0",
    "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "config": {
        "ethash": {},
        "chainId": 2600,
        "homesteadBlock": 0,
        "eip150Block": 0,
        "eip155Block": 0,
        "eip158Block": 0,
        "byzantiumBlock": 0,
        "constantinopleBlock": 0,
        "petersburgBlock": 0,
        "istanbulBlock": 0,
        "berlinBlock": 0,
        "londonBlock": 0,
        "terminalTotalDifficulty": 0,
        "terminalTotalDifficultyPassed": true,
        "shanghaiTime": 0
    }
}
"#;
    let genesis: Genesis = serde_json::from_str(custom_genesis).unwrap();
    Arc::new(genesis.into())
}
