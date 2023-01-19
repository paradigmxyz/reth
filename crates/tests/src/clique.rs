use enr::k256::ecdsa::SigningKey;
use ethers_core::{
    types::{
        transaction::eip2718::TypedTransaction, Address, Block, BlockNumber, Bytes, H256, U64,
    },
    utils::{ChainConfig, CliqueConfig, Genesis, GenesisAccount, Geth, GethInstance},
};
use ethers_middleware::SignerMiddleware;
use ethers_providers::{Middleware, Provider, Ws};
use ethers_signers::{LocalWallet, Signer, Wallet};
use reth_eth_wire::Status;
use reth_network::test_utils::{enr_to_peer_id, unused_port};
use reth_primitives::PeerId;
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    net::SocketAddr,
    time::Duration,
};
use tracing::trace;

/// Creates a chain config using the given chain id.
/// Funds the given address with max coins.
///
/// Enables all hard forks up to London at genesis.
pub(crate) fn genesis_funded(chain_id: u64, signer_addr: Address) -> Genesis {
    // set up a clique config with an instant sealing period and short (8 block) epoch
    let clique_config = CliqueConfig { period: 0, epoch: 8 };

    let config = ChainConfig {
        chain_id,
        eip155_block: Some(0),
        eip150_block: Some(0),
        eip158_block: Some(0),

        homestead_block: Some(0),
        byzantium_block: Some(0),
        constantinople_block: Some(0),
        petersburg_block: Some(0),
        istanbul_block: Some(0),
        muir_glacier_block: Some(0),
        berlin_block: Some(0),
        london_block: Some(0),
        clique: Some(clique_config),
        ..Default::default()
    };

    // fund account
    let mut alloc = HashMap::new();
    alloc.insert(
        signer_addr,
        GenesisAccount {
            balance: ethers_core::types::U256::MAX,
            nonce: None,
            code: None,
            storage: None,
        },
    );

    // put signer address in the extra data, padded by the required amount of zeros
    // Clique issue: https://github.com/ethereum/EIPs/issues/225
    // Clique EIP: https://eips.ethereum.org/EIPS/eip-225
    //
    // The first 32 bytes are vanity data, so we will populate it with zeros
    // This is followed by the signer address, which is 20 bytes
    // There are 65 bytes of zeros after the signer address, which is usually populated with the
    // proposer signature. Because the genesis does not have a proposer signature, it will be
    // populated with zeros.
    let extra_data_bytes = [&[0u8; 32][..], signer_addr.as_bytes(), &[0u8; 65][..]].concat();
    let extra_data = Bytes::from(extra_data_bytes);

    Genesis {
        config,
        alloc,
        difficulty: ethers_core::types::U256::one(),
        gas_limit: U64::from(5000000),
        extra_data,
        ..Default::default()
    }
}

/// Builder for setting up a [`Geth`](ethers_core::utils::Geth) instance configured with Clique
/// and a custom [`Genesis`](ethers_core::utils::Genesis).
///
/// If no private key is provided, a random one will be generated and configured as the Clique
/// signer.
/// In doing this, it will overwrite any `extra_data` field and configure clique in the provided
/// genesis. ```
/// use crate::CliqueGethBuilder;
/// use ethers_core::types::Bytes;
/// use k256::ecdsa::SigningKey;
///
/// let signing_key = SigningKey::random(&mut rand::thread_rng());
/// let private_key: Bytes = signing_key.to_bytes().to_vec().into();
///
/// let geth = CliqueGethBuilder::new()
///    .chain_id(1337)
///    .with_signer(private_key)
///    .data_dir("/tmp/clique-geth")
///    .build();
/// ```
#[derive(Debug)]
pub(crate) struct CliqueGethBuilder {
    chain_id: u64,
    signer: Option<Bytes>,
    genesis: Option<Genesis>,
    data_dir: Option<String>,
}

impl CliqueGethBuilder {
    /// Creates a new [`CliqueGethBuilder`].
    pub(crate) fn new() -> Self {
        Self { chain_id: 1337, signer: None, genesis: None, data_dir: None }
    }

    /// Sets the chain id for the [`Geth`](ethers_core::utils::Geth) instance.
    /// Defaults to `1337`.
    #[must_use]
    pub(crate) fn chain_id(mut self, chain_id: u64) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Sets the data dir for the [`Geth`](ethers_core::utils::Geth) instance.
    #[must_use]
    pub(crate) fn data_dir(mut self, data_dir: String) -> Self {
        self.data_dir = Some(data_dir);
        self
    }

    /// Sets the private key for the Clique signer.
    /// If no private key is provided, a random one will be generated. The generated key will also
    /// be funded with the maximum amount of coins in the genesis block.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn with_signer(mut self, private_key: Bytes) -> Self {
        self.signer = Some(private_key);
        self
    }

    /// Sets the genesis config for the [`Geth`](ethers_core::utils::Geth) instance.
    /// If no genesis is provided, one will be generated with forks up to London enabled at block
    /// zero.
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn with_genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Builds the [`Geth`](ethers_core::utils::Geth) instance.
    ///
    /// P2P functionality is enabled by default, as well as the `--allow-insecure-unlock` flag.
    /// Discovery is disabled by default.
    ///
    /// Returns the [`Geth`], a compatible [`Status`](reth_eth_wire::types::Status), the computed
    /// [`Genesis`](ethers_core::utils::Genesis) and the [`SigningKey`] created for signing blocks.
    pub(crate) async fn build(self) -> CliqueGethInstance {
        let signer = match self.signer {
            Some(private_key) => SigningKey::from_bytes(&private_key).expect("invalid private key"),
            None => SigningKey::random(&mut rand::thread_rng()),
        };

        // constructing the wallet is just for computing the address
        let wallet: LocalWallet = signer.clone().into();
        let our_address = wallet.address();

        // modify the provided genesis to configure clique, or generate a funded genesis
        let genesis = match self.genesis {
            Some(genesis) => {
                // overwrite the extraData field
                let mut genesis = genesis;

                // set up a clique config with an instant sealing period and short (8 block) epoch
                let clique_config = CliqueConfig { period: 0, epoch: 8 };
                genesis.config.clique = Some(clique_config);

                // set the extraData field
                let extra_data_bytes =
                    [&[0u8; 32][..], our_address.as_bytes(), &[0u8; 65][..]].concat();
                let extra_data = Bytes::from(extra_data_bytes);
                genesis.extra_data = extra_data;
                genesis
            }
            None => genesis_funded(self.chain_id, our_address),
        };

        let geth = if let Some(data_dir) = self.data_dir {
            Geth::new().data_dir(data_dir)
        } else {
            Geth::new()
        };

        let geth = geth
            .genesis(genesis.clone())
            .chain_id(self.chain_id)
            .p2p_port(unused_port())
            .disable_discovery()
            .insecure_unlock();

        // create a compatible status
        let status = Status::from(genesis.clone());

        CliqueGethInstance::new(geth, signer, status, genesis).await
    }
}

/// A [`Geth`](ethers_core::utils::Geth) instance configured with Clique and a custom
/// [`Genesis`](ethers_core::utils::Genesis).
///
/// This holds a [`SignerMiddleware`](ethers_middleware::signer_middleware::SignerMiddleware) for
/// enabling block production and creating transactions.
pub(crate) struct CliqueGethInstance {
    /// The spawned [`GethInstance`](ethers_core::utils::GethInstance).
    pub(crate) instance: GethInstance,

    /// The private key used for signing clique blocks and transactions.
    pub(crate) signer: SigningKey,

    /// The [`Status`](reth_eth_wire::Status) extracted from the configured geth
    /// [`Genesis`](ethers_core::utils::Genesis).
    pub(crate) status: Status,

    /// The local [`Genesis`](ethers_core::utils::Genesis) used to configure geth.
    pub(crate) genesis: Genesis,

    /// The ethers [`SignerMiddleware`](ethers_middleware::signer_middleware::SignerMiddleware)
    /// set up with the spawned geth instance.
    pub(crate) provider: SignerMiddleware<Provider<Ws>, Wallet<SigningKey>>,
}

impl CliqueGethInstance {
    /// Sets up a new [`SignerMiddleware`](ethers_middleware::signer_middleware::SignerMiddleware)
    /// for the [`Geth`](ethers_core::utils::Geth) instance and returns the
    /// [`CliqueGethInstance`].
    ///
    /// The signer is assumed to be the clique signer and the signer for any transactions sent for
    /// block production.
    ///
    /// This also spawns the geth instance.
    pub(crate) async fn new(
        geth: Geth,
        signer: SigningKey,
        status: Status,
        genesis: Genesis,
    ) -> Self {
        // spawn the geth instance
        let instance = geth.spawn();

        // create the signer
        let wallet: LocalWallet = signer.clone().into();

        // set up ethers provider
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), instance.port()).to_string();
        let provider = Provider::<Ws>::connect(format!("ws://{geth_endpoint}")).await.unwrap();
        let provider =
            SignerMiddleware::new_with_provider_chain(provider, wallet.clone()).await.unwrap();

        Self { instance, signer, status, genesis, provider }
    }

    /// Enable mining on the clique geth instance by importing and unlocking the signer account
    /// with the given password and starting mining.
    pub(crate) async fn enable_mining(&self, password: String) {
        let our_address = self.provider.address();

        // send the private key to geth and unlock it
        let key_bytes = self.signer.to_bytes().to_vec().into();
        trace!(
            private_key=%hex::encode(&key_bytes),
            "Importing private key"
        );

        let unlocked_addr =
            self.provider.import_raw_key(key_bytes, password.to_string()).await.unwrap();
        assert_eq!(unlocked_addr, our_address);

        let unlock_success =
            self.provider.unlock_account(our_address, password.to_string(), None).await.unwrap();
        assert!(unlock_success);

        // start mining?
        self.provider.start_mining(None).await.unwrap();

        // check that we are mining
        let mining = self.provider.mining().await.unwrap();
        assert!(mining);
    }

    /// Prints the logs of the [`Geth`](ethers_core::utils::Geth) instance in a new
    /// [`task`](tokio::task).
    pub(crate) fn print_logs(&mut self) {
        // take the stderr of the geth instance and print it
        let stderr = self.instance.stderr().unwrap();

        // print logs in a new task
        tokio::task::spawn(async move {
            let mut err_reader = BufReader::new(stderr);

            loop {
                let mut buf = String::new();
                if let Ok(line) = err_reader.read_line(&mut buf) {
                    if line == 0 {
                        tokio::time::sleep(Duration::from_nanos(1)).await;
                        continue
                    }
                    dbg!(buf);
                }
            }
        });
    }

    /// Signs and sends the given unsigned transactions sequentially, signing with the private key
    /// used to configure the [`CliqueGethInstance`].
    pub(crate) async fn send_requests(&self, txs: impl IntoIterator<Item = TypedTransaction>) {
        for tx in txs {
            self.provider.send_transaction(tx, None).await.unwrap();
        }
    }

    /// Returns the [`Geth`](ethers_core::utils::Geth) instance p2p port.
    pub(crate) fn p2p_port(&self) -> u16 {
        self.instance.p2p_port().unwrap()
    }

    /// Returns the [`Geth`](ethers_core::utils::Geth) instance [`PeerId`](reth_primitives::PeerId)
    /// by calling geth's `admin_nodeInfo`.
    pub(crate) async fn peer_id(&self) -> PeerId {
        enr_to_peer_id(self.provider.node_info().await.unwrap().enr)
    }

    /// Returns the genesis block of the [`Geth`](ethers_core::utils::Geth) instance by calling
    /// geth's `eth_getBlock`.
    pub(crate) async fn genesis(&self) -> Block<H256> {
        self.provider
            .get_block(BlockNumber::Earliest)
            .await
            .unwrap()
            .expect("a genesis block should exist")
    }

    /// Returns the chain tip of the [`Geth`](ethers_core::utils::Geth) instance by calling
    /// geth's `eth_getBlock`.
    pub(crate) async fn tip(&self) -> Block<H256> {
        self.provider
            .get_block(BlockNumber::Latest)
            .await
            .unwrap()
            .expect("a chain tip should exist")
    }

    /// Returns the chain tip hash of the [`Geth`](ethers_core::utils::Geth) instance by calling
    /// from geth's `eth_getBlock`.
    pub(crate) async fn tip_hash(&self) -> reth_primitives::H256 {
        self.tip().await.hash.unwrap().0.into()
    }

    /// Returns the genesis hash of the [`Geth`](ethers_core::utils::Geth) instance by calling
    /// geth's `eth_getBlock`.
    #[allow(dead_code)]
    pub(crate) async fn genesis_hash(&self) -> reth_primitives::H256 {
        self.genesis().await.hash.unwrap().0.into()
    }
}
