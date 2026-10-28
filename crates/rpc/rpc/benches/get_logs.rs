//! Compares `eth_getLogs` with `eth_getLogsV2` and its field selection, by speed and response size.
//!
//! The data is a fixed range of real mainnet blocks. The first run fetches the blocks and receipts
//! from a public RPC and writes them into an on-disk reth database, later runs reuse it.
//!
//! Environment variables:
//! - `GET_LOGS_BENCH_RPC`: RPC to fetch from, defaults to `https://ethereum-rpc.publicnode.com`.
//! - `GET_LOGS_BENCH_DATADIR`: where the database lives, defaults to `target/get-logs-bench`.
//! - `GET_LOGS_BENCH_REVERSE`: if set, runs the variants of each query in reverse order.

#![expect(missing_docs)]

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{address, b256, Address, Bytes, B256, U64};
use alloy_rpc_client::{ClientBuilder, RpcClient};
use alloy_rpc_types_eth::{Filter, TransactionReceipt};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use futures::{stream, StreamExt, TryStreamExt};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_db::{init_db, mdbx::DatabaseArguments, ClientVersion, DatabaseEnv};
use reth_db_common::init::init_genesis;
use reth_ethereum_primitives::{Block, Receipt, TransactionSigned};
use reth_evm_ethereum::EthEvmConfig;
use reth_execution_types::ExecutionOutcome;
use reth_network_api::noop::NoopNetwork;
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{
    providers::{BlockchainProvider, RocksDBBuilder, StaticFileProviderBuilder},
    test_utils::MockNodeTypesWithDB,
    BlockWriter, ProviderFactory,
};
use reth_rpc::{EthApi, EthFilter};
use reth_rpc_eth_api::EthFilterApiServer;
use reth_rpc_eth_types::{
    logs_v2::{Direction, LogBlock, LogField, LogV2, LogsV2Filter, LogsV2Result, LogsV2Start},
    EthFilterConfig,
};
use reth_tasks::Runtime;
use reth_transaction_pool::noop::NoopTransactionPool;
use serde::Serialize;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::runtime::Runtime as TokioRuntime;

/// Last block of the benchmark range.
const LAST_BLOCK: u64 = 26_280_000;
/// Number of blocks in the benchmark range.
const BLOCK_COUNT: u64 = 5_000;
/// First block of the benchmark range.
const FIRST_BLOCK: u64 = LAST_BLOCK - BLOCK_COUNT + 1;
/// Number of blocks written per database transaction while seeding.
const SEED_BATCH: u64 = 100;
/// Number of concurrent requests while seeding.
const SEED_CONCURRENCY: usize = 8;

const USDC: Address = address!("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
const ENS_REGISTRY: Address = address!("0x00000000000C2E074eC69A0dFb2997BA6C7d2e1e");
const TRANSFER_TOPIC: B256 =
    b256!("0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");

type Factory = ProviderFactory<MockNodeTypesWithDB<Arc<DatabaseEnv>>>;
type Provider = BlockchainProvider<MockNodeTypesWithDB<Arc<DatabaseEnv>>>;

/// A query, run as `eth_getLogs` and as `eth_getLogsV2` with each field selection.
struct Scenario {
    name: &'static str,
    filter: Filter,
    from_block: u64,
    to_block: u64,
}

/// A way to serve a scenario.
#[derive(Clone, Copy)]
enum Variant {
    V1,
    V2(Option<&'static [LogField]>),
}

impl Variant {
    const ALL: [(&'static str, Self); 5] = [
        ("getLogs", Self::V1),
        ("v2_all_fields", Self::V2(None)),
        (
            "v2_no_tx_hash",
            Self::V2(Some(&[
                LogField::Address,
                LogField::Topics,
                LogField::Data,
                LogField::BlockTimestamp,
                LogField::TransactionIndex,
            ])),
        ),
        ("v2_topics_data", Self::V2(Some(&[LogField::Topics, LogField::Data]))),
        ("v2_positions", Self::V2(Some(&[]))),
    ];
}

fn scenarios() -> Vec<Scenario> {
    // Ranges end at least 1_000 blocks before the tip, so they are read from the database rather
    // than the RPC cache, except for the last scenario.
    let old_end = LAST_BLOCK - 1_000;
    vec![
        Scenario {
            name: "usdc_transfers_1000_blocks",
            filter: Filter::new().address(USDC).event_signature(TRANSFER_TOPIC),
            from_block: old_end - 999,
            to_block: old_end,
        },
        Scenario {
            name: "all_logs_100_blocks",
            filter: Filter::new(),
            from_block: old_end - 99,
            to_block: old_end,
        },
        Scenario {
            name: "ens_registry_4000_blocks",
            filter: Filter::new().address(ENS_REGISTRY),
            from_block: old_end - 3_999,
            to_block: old_end,
        },
        Scenario {
            name: "all_logs_100_blocks_at_tip",
            filter: Filter::new(),
            from_block: LAST_BLOCK - 99,
            to_block: LAST_BLOCK,
        },
    ]
}

/// Serves `variant` of `scenario` and returns the JSON response body and the number of logs.
async fn serve<Eth>(
    filter: &EthFilter<Eth>,
    scenario: &Scenario,
    variant: Variant,
) -> (Vec<u8>, usize)
where
    EthFilter<Eth>: EthFilterApiServer<alloy_rpc_types_eth::Transaction, alloy_rpc_types_eth::Log>,
    Eth: reth_rpc_eth_api::EthApiTypes,
{
    match variant {
        Variant::V1 => {
            let request =
                scenario.filter.clone().from_block(scenario.from_block).to_block(scenario.to_block);
            let logs = EthFilterApiServer::logs(filter, request).await.unwrap();
            (serde_json::to_vec(&logs).unwrap(), logs.len())
        }
        Variant::V2(fields) => {
            let request = LogsV2Filter {
                start: LogsV2Start::FromBlock(BlockNumberOrTag::Number(scenario.from_block)),
                to_block: Some(BlockNumberOrTag::Number(scenario.to_block)),
                direction: Direction::Asc,
                filter: scenario.filter.clone(),
                max_logs: None,
                max_blocks: None,
                fields: fields.map(<[LogField]>::to_vec),
            };
            let result = EthFilterApiServer::logs_v2(filter, request).await.unwrap();
            assert!(
                result.next_cursor.log_index.is_none() &&
                    result.next_cursor.block_number == scenario.to_block
            );
            (serde_json::to_vec(&result).unwrap(), result.logs.len())
        }
    }
}

fn get_logs(c: &mut Criterion) {
    let tokio = TokioRuntime::new().unwrap();
    let provider = tokio.block_on(open_or_seed());
    let eth_api = EthApi::builder(
        provider,
        NoopTransactionPool::default(),
        NoopNetwork::default(),
        EthEvmConfig::mainnet(),
    )
    .build();
    let filter = {
        let _guard = tokio.enter();
        EthFilter::new(
            eth_api,
            EthFilterConfig {
                max_blocks_per_filter: None,
                max_logs_per_response: None,
                ..Default::default()
            },
            Runtime::test(),
        )
    };

    let scenarios = scenarios();
    print_sizes(&tokio, &filter, &scenarios);
    dedup(c, &tokio, &filter, &scenarios);

    for scenario in &scenarios {
        let mut group = c.benchmark_group(scenario.name);
        group.sample_size(10).warm_up_time(Duration::from_secs(1));
        let mut variants = Variant::ALL;
        // Alternating the order between runs shows whether a variant is affected by the one
        // before it.
        if std::env::var_os("GET_LOGS_BENCH_REVERSE").is_some() {
            variants.reverse();
        }
        for (name, variant) in variants {
            group.bench_function(BenchmarkId::from_parameter(name), |b| {
                b.iter(|| tokio.block_on(serve(&filter, scenario, variant)))
            });
        }
        group.finish();
    }
}

/// Prints the response size of every variant of every scenario, raw and gzipped.
fn print_sizes<Eth>(tokio: &TokioRuntime, filter: &EthFilter<Eth>, scenarios: &[Scenario])
where
    EthFilter<Eth>: EthFilterApiServer<alloy_rpc_types_eth::Transaction, alloy_rpc_types_eth::Log>,
    Eth: reth_rpc_eth_api::EthApiTypes,
{
    println!();
    println!(
        "{:<28} {:<16} {:>8} {:>14} {:>14}",
        "scenario", "variant", "logs", "bytes", "gzip bytes"
    );
    for scenario in scenarios {
        for (name, variant) in Variant::ALL {
            let (body, logs) = tokio.block_on(serve(filter, scenario, variant));
            let gzipped = gzip_len(&body);
            println!(
                "{:<28} {:<16} {:>8} {:>14} {:>14}",
                scenario.name,
                name,
                logs,
                body.len(),
                gzipped
            );
        }
    }
    println!();
}

/// Compares the v2 response, which states block fields once in a list next to the logs, with flat
/// logs and logs grouped by block, by size and serialization time. All v2 shapes carry the same
/// information as `v2_all_fields`.
fn dedup<Eth>(
    c: &mut Criterion,
    tokio: &TokioRuntime,
    filter: &EthFilter<Eth>,
    scenarios: &[Scenario],
) where
    EthFilter<Eth>: EthFilterApiServer<alloy_rpc_types_eth::Transaction, alloy_rpc_types_eth::Log>,
    Eth: reth_rpc_eth_api::EthApiTypes,
{
    let mut sizes = Vec::new();
    for scenario in scenarios {
        let request =
            scenario.filter.clone().from_block(scenario.from_block).to_block(scenario.to_block);
        let v1_logs = tokio.block_on(EthFilterApiServer::logs(filter, request)).unwrap();
        let v2 = tokio
            .block_on(EthFilterApiServer::logs_v2(
                filter,
                LogsV2Filter {
                    start: LogsV2Start::FromBlock(BlockNumberOrTag::Number(scenario.from_block)),
                    to_block: Some(BlockNumberOrTag::Number(scenario.to_block)),
                    direction: Direction::Asc,
                    filter: scenario.filter.clone(),
                    max_logs: None,
                    max_blocks: None,
                    fields: None,
                },
            ))
            .unwrap();
        let v2_logs = V2Logs { logs: &v2.logs, blocks: &v2.blocks };
        let flat = flatten(&v2);
        let by_block = group_by_block(&v2);
        let mip16 = mip16_example(&v2);

        let shapes: [(&str, &dyn Fn() -> Vec<u8>); 5] = [
            ("getLogs_flat", &|| serde_json::to_vec(&v1_logs).unwrap()),
            ("v2_flat", &|| serde_json::to_vec(&flat).unwrap()),
            ("v2_blocks_list", &|| serde_json::to_vec(&v2_logs).unwrap()),
            ("v2_by_block", &|| serde_json::to_vec(&by_block).unwrap()),
            ("mip16_example", &|| serde_json::to_vec(&mip16).unwrap()),
        ];

        let mut group = c.benchmark_group(format!("serialize_{}", scenario.name));
        group.sample_size(10).warm_up_time(Duration::from_secs(1));
        for (name, serialize) in shapes {
            let body = serialize();
            sizes.push((scenario.name, name, body.len(), gzip_len(&body)));
            group.bench_function(BenchmarkId::from_parameter(name), |b| b.iter(serialize));
        }
        group.finish();
    }

    println!();
    println!("{:<28} {:<20} {:>14} {:>14}", "scenario", "shape", "bytes", "gzip bytes");
    for (scenario, shape, bytes, gzipped) in sizes {
        println!("{scenario:<28} {shape:<20} {bytes:>14} {gzipped:>14}");
    }
    println!();
}

fn gzip_len(body: &[u8]) -> usize {
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(body).unwrap();
    gzip.finish().unwrap().len()
}

/// The logs and blocks of a v2 response, without the cursor and block references.
#[derive(Serialize)]
struct V2Logs<'a> {
    logs: &'a [LogV2],
    blocks: &'a [LogBlock],
}

/// A log with its block fields, as v2 returned them before they moved to the blocks list.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FlatLog<'a> {
    block_number: U64,
    block_hash: B256,
    log_index: U64,
    address: Address,
    topics: &'a [B256],
    data: &'a Bytes,
    block_timestamp: U64,
    transaction_hash: B256,
    transaction_index: U64,
}

/// Logs of one block, with the block fields stated once.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockLogs<'a> {
    block_number: U64,
    block_hash: B256,
    block_timestamp: U64,
    logs: Vec<LogInBlock<'a>>,
}

/// A log without its block fields.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LogInBlock<'a> {
    log_index: U64,
    address: Address,
    topics: &'a [B256],
    data: &'a Bytes,
    transaction_hash: B256,
    transaction_index: U64,
}

/// Returns the logs of each block of a v2 response together with the block.
fn logs_by_block(v2: &LogsV2Result) -> impl Iterator<Item = (&LogBlock, &[LogV2])> {
    v2.blocks.iter().zip(v2.logs.chunk_by(|a, b| a.block_number == b.block_number))
}

fn flatten(v2: &LogsV2Result) -> Vec<FlatLog<'_>> {
    logs_by_block(v2)
        .flat_map(|(block, logs)| {
            logs.iter().map(|log| FlatLog {
                block_number: U64::from(log.block_number),
                block_hash: block.hash,
                log_index: U64::from(log.log_index),
                address: log.address.unwrap(),
                topics: log.topics.as_deref().unwrap(),
                data: log.data.as_ref().unwrap(),
                block_timestamp: U64::from(block.timestamp.unwrap()),
                transaction_hash: log.transaction_hash.unwrap(),
                transaction_index: U64::from(log.transaction_index.unwrap()),
            })
        })
        .collect()
}

fn group_by_block(v2: &LogsV2Result) -> Vec<BlockLogs<'_>> {
    logs_by_block(v2)
        .map(|(block, logs)| BlockLogs {
            block_number: U64::from(block.number),
            block_hash: block.hash,
            block_timestamp: U64::from(block.timestamp.unwrap()),
            logs: logs
                .iter()
                .map(|log| LogInBlock {
                    log_index: U64::from(log.log_index),
                    address: log.address.unwrap(),
                    topics: log.topics.as_deref().unwrap(),
                    data: log.data.as_ref().unwrap(),
                    transaction_hash: log.transaction_hash.unwrap(),
                    transaction_index: U64::from(log.transaction_index.unwrap()),
                })
                .collect(),
        })
        .collect()
}

/// The `data` of an MIP-16 `eth_queryLogs` response with the field selection of the MIP's example:
/// logs with `blockNumber`, `logIndex`, `address`, `data` and `topics`, and the `number` and
/// `timestamp` of each block that has a log.
///
/// See <https://github.com/monad-crypto/MIPs/pull/89>.
#[derive(Serialize)]
struct Mip16Response<'a> {
    data: Mip16Data<'a>,
}

#[derive(Serialize)]
struct Mip16Data<'a> {
    logs: Vec<Mip16Log<'a>>,
    blocks: Vec<Mip16Block>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Mip16Log<'a> {
    block_number: U64,
    log_index: U64,
    address: Address,
    data: &'a Bytes,
    topics: &'a [B256],
}

#[derive(Serialize)]
struct Mip16Block {
    number: U64,
    timestamp: U64,
}

fn mip16_example(v2: &LogsV2Result) -> Mip16Response<'_> {
    Mip16Response {
        data: Mip16Data {
            logs: v2
                .logs
                .iter()
                .map(|log| Mip16Log {
                    block_number: U64::from(log.block_number),
                    log_index: U64::from(log.log_index),
                    address: log.address.unwrap(),
                    data: log.data.as_ref().unwrap(),
                    topics: log.topics.as_deref().unwrap(),
                })
                .collect(),
            blocks: v2
                .blocks
                .iter()
                .map(|block| Mip16Block {
                    number: U64::from(block.number),
                    timestamp: U64::from(block.timestamp.unwrap()),
                })
                .collect(),
        },
    }
}

/// Opens the benchmark database, fetching and writing the blocks first if it is incomplete.
async fn open_or_seed() -> Provider {
    let datadir =
        std::env::var_os("GET_LOGS_BENCH_DATADIR").map(PathBuf::from).unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/get-logs-bench")
        });
    let datadir = datadir.join(format!("{FIRST_BLOCK}-{LAST_BLOCK}"));
    let complete = datadir.join("complete");
    let seeded = complete.exists();
    if !seeded && datadir.exists() {
        std::fs::remove_dir_all(&datadir).unwrap();
    }

    let factory = open_factory(&datadir);
    if !seeded {
        init_genesis(&factory).unwrap();
        let rpc = std::env::var("GET_LOGS_BENCH_RPC")
            .unwrap_or_else(|_| "https://ethereum-rpc.publicnode.com".to_owned());
        seed(&factory, &ClientBuilder::default().http(rpc.parse().unwrap())).await;
        std::fs::write(&complete, []).unwrap();
    }
    BlockchainProvider::new(factory).unwrap()
}

fn open_factory(datadir: &Path) -> Factory {
    let static_files = datadir.join("static_files");
    std::fs::create_dir_all(&static_files).unwrap();
    let db = init_db(datadir.join("db"), DatabaseArguments::new(ClientVersion::default())).unwrap();

    // The database's genesis sits right before the first block and is never queried, so it has no
    // state.
    let mut genesis = MAINNET.genesis.clone();
    genesis.number = Some(FIRST_BLOCK - 1);
    genesis.alloc = Default::default();
    let chain_spec = Arc::new(ChainSpecBuilder::mainnet().genesis(genesis).build());

    ProviderFactory::new(
        Arc::new(db),
        chain_spec,
        StaticFileProviderBuilder::read_write(static_files)
            .with_genesis_block_number(FIRST_BLOCK - 1)
            .build()
            .unwrap(),
        RocksDBBuilder::new(datadir.join("rocksdb")).with_default_tables().build().unwrap(),
        Runtime::test(),
    )
    .unwrap()
}

/// Fetches the benchmark range and writes it to `factory`.
async fn seed(factory: &Factory, rpc: &RpcClient) {
    let mut from = FIRST_BLOCK;
    while from <= LAST_BLOCK {
        let to = (from + SEED_BATCH - 1).min(LAST_BLOCK);
        let fetched = stream::iter(from..=to)
            .map(|number| fetch_block(rpc, number))
            .buffered(SEED_CONCURRENCY)
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let (blocks, receipts): (Vec<_>, Vec<_>) = fetched.into_iter().unzip();

        let provider = factory.provider_rw().unwrap();
        let requests = vec![Default::default(); blocks.len()];
        provider
            .append_blocks_with_state(
                blocks,
                &ExecutionOutcome {
                    bundle: Default::default(),
                    receipts,
                    first_block: from,
                    requests,
                },
                Default::default(),
            )
            .unwrap();
        provider.commit().unwrap();

        eprintln!("seeded blocks {from}..={to}");
        from = to + 1;
    }
}

/// Fetches a block with its senders and receipts.
async fn fetch_block(
    rpc: &RpcClient,
    number: u64,
) -> Result<(RecoveredBlock<Block>, Vec<Receipt>), alloy_transport::TransportError> {
    let tag = BlockNumberOrTag::Number(number);
    let (block, receipts) = tokio::try_join!(
        rpc.request::<_, alloy_rpc_types_eth::Block<alloy_rpc_types_eth::Transaction<TransactionSigned>>>(
            "eth_getBlockByNumber",
            (tag, true)
        ),
        rpc.request::<_, Vec<TransactionReceipt>>("eth_getBlockReceipts", (tag,)),
    )?;

    let hash = block.header.hash;
    let (transactions, senders): (Vec<_>, Vec<_>) = block
        .transactions
        .into_transactions()
        .map(|tx| {
            let (tx, sender) = tx.inner.into_parts();
            (tx, sender)
        })
        .unzip();
    let block = Block {
        header: block.header.inner,
        body: reth_ethereum_primitives::BlockBody {
            transactions,
            ommers: Vec::new(),
            withdrawals: block.withdrawals,
        },
    };
    let receipts = receipts
        .into_iter()
        .map(|receipt| Receipt {
            tx_type: receipt.inner.tx_type(),
            success: receipt.inner.status(),
            cumulative_gas_used: receipt.inner.cumulative_gas_used(),
            logs: receipt.inner.logs().iter().map(|log| log.inner.clone()).collect(),
        })
        .collect();
    Ok((RecoveredBlock::new(block, senders, hash), receipts))
}

criterion_group!(benches, get_logs);
criterion_main!(benches);
