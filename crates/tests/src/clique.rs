/// Contains builder types and clique-related test setup helpers for geth integration tests.

use reth_net_test_utils::{Testnet, NetworkEventStream, PeerConfig};
use enr::{k256::ecdsa::SigningKey, Enr, EnrPublicKey};
use ethers_core::{
    types::{Address, Block, Bytes, U64},
    utils::{ChainConfig, CliqueConfig, Genesis, GenesisAccount, Geth},
};
use ethers_middleware::SignerMiddleware;
use ethers_providers::{Http, Middleware, Provider};
use ethers_signers::{LocalWallet, Signer};
use futures::StreamExt;
use reth_discv4::{bootnodes::mainnet_nodes, Discv4Config};
use reth_eth_wire::{DisconnectReason, EthVersion, Status};
use reth_interfaces::{
    p2p::headers::client::{HeadersClient, HeadersRequest},
    sync::{SyncState, SyncStateUpdater},
};
use reth_net_common::ban_list::BanList;
use reth_network::{NetworkConfig, NetworkEvent, NetworkHandle, NetworkManager, PeersConfig};
use reth_primitives::{
    proofs::genesis_state_root, Chain, ForkHash, ForkId, Header, HeadersDirection, NodeRecord,
    PeerId, H160, H256, INITIAL_BASE_FEE,
};
use reth_provider::test_utils::NoopProvider;
use secp256k1::SecretKey;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::task;

/// Extracts the genesis block header from an ethers [`Genesis`](ethers_core::utils::Genesis).
fn genesis_header(genesis: &Genesis) -> Header {
    let genesis_alloc = genesis.alloc.clone();
    // convert to reth genesis alloc map
    let reth_alloc = genesis_alloc
        .into_iter()
        .map(|(address, account)| (address.0.into(), convert_genesis_account(&account)))
        .collect::<HashMap<H160, _>>();

    Header {
        gas_limit: genesis.gas_limit.as_u64(),
        difficulty: genesis.difficulty.into(),
        nonce: genesis.nonce.as_u64(),
        extra_data: genesis.extra_data.0.clone(),
        state_root: genesis_state_root(reth_alloc),
        timestamp: genesis.timestamp.as_u64(),
        mix_hash: genesis.mix_hash.0.into(),
        beneficiary: genesis.coinbase.0.into(),
        base_fee_per_gas: genesis.config.london_block.map(|_| INITIAL_BASE_FEE),
        ..Default::default()
    }
}

/// Converts an ethers [`GenesisAccount`](ethers_core::utils::GenesisAccount) to a reth
/// [`GenesisAccount`](reth_primitives::GenesisAccount).
fn convert_genesis_account(genesis_account: &GenesisAccount) -> reth_primitives::GenesisAccount {
    reth_primitives::GenesisAccount {
        balance: genesis_account.balance.into(),
        nonce: genesis_account.nonce,
        code: genesis_account.code.as_ref().map(|code| code.0.clone().into()),
        storage: genesis_account.storage.as_ref().map(|storage| {
            storage.clone().into_iter().map(|(k, v)| (k.0.into(), v.0.into())).collect()
        }),
    }
}

/// Obtains a [`Header`](reth_primitives::Header) from an ethers
/// [`Block`](ethers_core::types::Block).
fn block_to_header(block: Block<ethers_core::types::H256>) -> Header {
    Header {
        number: block.number.unwrap().as_u64(),
        gas_limit: block.gas_limit.as_u64(),
        difficulty: block.difficulty.into(),
        nonce: block.nonce.unwrap().to_low_u64_be(),
        extra_data: block.extra_data.0,
        state_root: block.state_root.0.into(),
        timestamp: block.timestamp.as_u64(),
        mix_hash: block.mix_hash.unwrap().0.into(),
        beneficiary: block.author.unwrap().0.into(),
        base_fee_per_gas: block.base_fee_per_gas.map(|fee| fee.as_u64()),
        ..Default::default()
    }
}

/// Obtains a [`ForkId`](reth_primitives::ForkId) from an ethers
/// [`Genesis`](ethers_core::utils::Genesis).
fn extract_fork_hash(genesis: &Genesis) -> ForkHash {
    // first create header and get hash
    let sealed_header = genesis_header(genesis).seal();
    let fork_blocks = extract_fork_blocks(genesis);
    let chain_forkhash =
        fork_blocks.iter().fold(ForkHash::from(sealed_header.hash()), |acc, block| acc + *block);

    chain_forkhash
}

/// Obtains an initial [`Status`](reth_eth_wire::Status) from an ethers
/// [`Genesis`](ethers_core::utils::Genesis).
///
/// Sets the `blockhash` and `genesis` fields to the genesis block hash, and initializes the
/// `total_difficulty` as zero.
fn extract_status(genesis: &Genesis) -> Status {
    let sealed_header = genesis_header(genesis).seal();
    let chain_forkhash = extract_fork_hash(genesis);

    Status {
        version: EthVersion::Eth67 as u8,
        chain: Chain::Id(genesis.config.chain_id),
        total_difficulty: genesis.difficulty.into(),
        blockhash: sealed_header.hash(),
        genesis: sealed_header.hash(),
        forkid: ForkId { hash: chain_forkhash, next: 0 },
    }
}

/// Obtains the list of fork block numbers in order from an ethers
/// [`Genesis`](ethers_core::utils::Genesis).
///
/// This should be the same as [Geth's `gather_forks`
/// method](https://github.com/ethereum/go-ethereum/blob/6c149fd4ad063f7c24d726a73bc0546badd1bc73/core/forkid/forkid.go#L215).
fn extract_fork_blocks(genesis: &Genesis) -> Vec<u64> {
    // will just put each consecutive fork in a vec
    let mut fork_blocks_opt = vec![
        genesis.config.homestead_block,
        genesis.config.dao_fork_block,
        genesis.config.eip150_block,
        genesis.config.eip155_block,
        genesis.config.eip158_block,
        genesis.config.byzantium_block,
        genesis.config.constantinople_block,
        genesis.config.petersburg_block,
        genesis.config.istanbul_block,
        genesis.config.muir_glacier_block,
        genesis.config.berlin_block,
        genesis.config.london_block,
        genesis.config.arrow_glacier_block,
        genesis.config.gray_glacier_block,
        genesis.config.merge_netsplit_block,
        genesis.config.shanghai_block,
        genesis.config.cancun_block,
    ];

    // filter out the None values
    fork_blocks_opt.retain(|block| block.is_some());

    // safely use unwrap (the vec is now guaranteed to have no None values)
    let mut fork_blocks: Vec<u64> = fork_blocks_opt.iter().map(|block| block.unwrap()).collect();

    // Sort the fork block numbers to permit chronological XOR
    fork_blocks.sort();

    // Deduplicate block numbers applying multiple forks (each block number should only be
    // represented once)
    fork_blocks_opt.dedup();

    // Skip any forks in block 0, that's the genesis ruleset
    fork_blocks.retain(|block| *block != 0);
    fork_blocks
}

/// Starts the reth pipeline with the given config, consensus, db, and fetch client.
/// .... TODO: doc
/// TODO: need to figure out where to put this test as it will import every part of the node.
async fn start_reth(network: NetworkHandle) {
    let _fetch_client = Arc::new(network.fetch_client().await.unwrap());
    // let mut pipeline = reth_stages::Pipeline::default()
    //     .with_sync_state_updater(network.clone())
    //     .push(HeaderStage {
    //         downloader: headers::linear::LinearDownloadBuilder::default()
    //             .batch_size(config.stages.headers.downloader_batch_size)
    //             .retries(config.stages.headers.downloader_retries)
    //             .build(consensus.clone(), fetch_client.clone()),
    //         consensus: consensus.clone(),
    //         client: fetch_client.clone(),
    //         network_handle: network.clone(),
    //         commit_threshold: config.stages.headers.commit_threshold,
    //         metrics: HeaderMetrics::default(),
    //     })
    //     .push(TotalDifficultyStage {
    //         commit_threshold: config.stages.total_difficulty.commit_threshold,
    //     })
    //     .push(BodyStage {
    //         downloader: Arc::new(
    //             bodies::concurrent::ConcurrentDownloader::new(
    //                 fetch_client.clone(),
    //                 consensus.clone(),
    //             )
    //             .with_batch_size(config.stages.bodies.downloader_batch_size)
    //             .with_retries(config.stages.bodies.downloader_retries)
    //             .with_concurrency(config.stages.bodies.downloader_concurrency),
    //         ),
    //         consensus: consensus.clone(),
    //         commit_threshold: config.stages.bodies.commit_threshold,
    //     })
    //     .push(SenderRecoveryStage {
    //         batch_size: config.stages.sender_recovery.batch_size,
    //         commit_threshold: config.stages.sender_recovery.commit_threshold,
    //     })
    //     .push(ExecutionStage {
    //         config: ExecutorConfig::new_ethereum(),
    //         commit_threshold: config.stages.execution.commit_threshold,
    //     });

    // if let Some(tip) = self.tip {
    //     debug!("Tip manually set: {}", tip);
    //     consensus.notify_fork_choice_state(ForkchoiceState {
    //         head_block_hash: tip,
    //         safe_block_hash: tip,
    //         finalized_block_hash: tip,
    //     })?;
    // }

    // // Run pipeline
    // info!("Starting pipeline");
    // pipeline.run(db.clone()).await?;
}

/// Creates a chain config using the given chain id.
/// Funds the given address with max coins.
///
/// Enables all hard forks up to London at genesis.
fn genesis_funded(chain_id: u64, signer_addr: Address) -> Genesis {
    // set up a clique config with a short (1s) period and short (8 block) epoch
    let clique_config = CliqueConfig { period: 1, epoch: 8 };

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
