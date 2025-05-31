#![warn(unused_crate_dependencies)]

use alloy_primitives::{Address, B256};
use reth_ethereum::{
    chainspec::ChainSpecBuilder,
    node::EthereumNode,
    primitives::{AlloyBlockHeader, SealedBlock, SealedHeader},
    provider::{
        providers::ReadOnlyConfig, AccountReader, BlockReader, BlockSource, HeaderProvider,
        ReceiptProvider, StateProvider, TransactionsProvider,
    },
    rpc::eth::primitives::Filter,
    TransactionSigned,
};

// Providers are zero cost abstractions on top of an opened MDBX Transaction
// exposing a familiar API to query the chain's information without requiring knowledge
// of the inner tables.
//
// These abstractions do not include any caching and the user is responsible for doing that.
// Other parts of the code which include caching are parts of the `EthApi` abstraction.
fn main() -> eyre::Result<()> {
    // The path to data directory, e.g. "~/.local/reth/share/mainnet"
    let datadir = std::env::var("RETH_DATADIR")?;

    // Instantiate a provider factory for Ethereum mainnet using the provided datadir path.
    let spec = ChainSpecBuilder::mainnet().build();
    let factory = EthereumNode::provider_factory_builder()
        .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

    // This call opens a RO transaction on the database. To write to the DB you'd need to call
    // the `provider_rw` function and look for the `Writer` variants of the traits.
    let provider = factory.provider()?;

    // Run basic queries against the DB
    let block_num = 100;
    header_provider_example(&provider, block_num)?;
    block_provider_example(&provider, block_num)?;
    txs_provider_example(&provider)?;
    receipts_provider_example(&provider)?;

    // Closes the RO transaction opened in the `factory.provider()` call. This is optional and
    // would happen anyway at the end of the function scope.
    drop(provider);

    // Run the example against latest state
    state_provider_example(factory.latest()?)?;

    // Run it with historical state
    state_provider_example(factory.history_by_block_number(block_num)?)?;

    Ok(())
}

/// The `HeaderProvider` allows querying the headers-related tables.
fn header_provider_example<T: HeaderProvider>(provider: T, number: u64) -> eyre::Result<()> {
    // Can query the header by number
    let header = provider.header_by_number(number)?.ok_or(eyre::eyre!("header not found"))?;

    // We can convert a header to a sealed header which contains the hash w/o needing to re-compute
    // it every time.
    let sealed_header = SealedHeader::seal_slow(header);

    // Can also query the header by hash!
    let header_by_hash =
        provider.header(&sealed_header.hash())?.ok_or(eyre::eyre!("header by hash not found"))?;
    assert_eq!(sealed_header.header(), &header_by_hash);

    // The header's total difficulty is stored in a separate table, so we have a separate call for
    // it. This is not needed for post PoS transition chains.
    let td = provider.header_td_by_number(number)?.ok_or(eyre::eyre!("header td not found"))?;
    assert!(!td.is_zero());

    // Can query headers by range as well, already sealed!
    let headers = provider.sealed_headers_range(100..200)?;
    assert_eq!(headers.len(), 100);

    Ok(())
}

/// The `TransactionsProvider` allows querying transaction-related information
fn txs_provider_example<T: TransactionsProvider<Transaction = TransactionSigned>>(
    provider: T,
) -> eyre::Result<()> {
    // Try the 5th tx
    let txid = 5;

    // Query a transaction by its primary ordered key in the db
    let tx = provider.transaction_by_id(txid)?.ok_or(eyre::eyre!("transaction not found"))?;

    // Can query the tx by hash
    let tx_by_hash =
        provider.transaction_by_hash(*tx.tx_hash())?.ok_or(eyre::eyre!("txhash not found"))?;
    assert_eq!(tx, tx_by_hash);

    // Can query the tx by hash with info about the block it was included in
    let (tx, meta) = provider
        .transaction_by_hash_with_meta(*tx.tx_hash())?
        .ok_or(eyre::eyre!("txhash not found"))?;
    assert_eq!(*tx.tx_hash(), meta.tx_hash);

    // Can reverse lookup the key too
    let id = provider.transaction_id(*tx.tx_hash())?.ok_or(eyre::eyre!("txhash not found"))?;
    assert_eq!(id, txid);

    // Can find the block of a transaction given its key
    let _block = provider.transaction_block(txid)?;

    // Can query the txs in the range [100, 200)
    let _txs_by_tx_range = provider.transactions_by_tx_range(100..200)?;
    // Can query the txs in the _block_ range [100, 200)]
    let _txs_by_block_range = provider.transactions_by_block_range(100..200)?;

    Ok(())
}

/// The `BlockReader` allows querying the headers-related tables.
fn block_provider_example<T: BlockReader<Block = reth_ethereum::Block>>(
    provider: T,
    number: u64,
) -> eyre::Result<()> {
    // Can query a block by number
    let block = provider.block(number.into())?.ok_or(eyre::eyre!("block num not found"))?;
    assert_eq!(block.number, number);

    // Can query a block with its senders, this is useful when you'd want to execute a block and do
    // not want to manually recover the senders for each transaction (as each transaction is
    // stored on disk with its v,r,s but not its `from` field.).
    let block = provider.block(number.into())?.ok_or(eyre::eyre!("block num not found"))?;

    // Can seal the block to cache the hash, like the Header above.
    let sealed_block = SealedBlock::seal_slow(block.clone());

    // Can also query the block by hash directly
    let block_by_hash = provider
        .block_by_hash(sealed_block.hash())?
        .ok_or(eyre::eyre!("block by hash not found"))?;
    assert_eq!(block, block_by_hash);

    // Or by relying in the internal conversion
    let block_by_hash2 = provider
        .block(sealed_block.hash().into())?
        .ok_or(eyre::eyre!("block by hash not found"))?;
    assert_eq!(block, block_by_hash2);

    // Or you can also specify the datasource. For this provider this always return `None`, but
    // the blockchain tree is also able to access pending state not available in the db yet.
    let block_by_hash3 = provider
        .find_block_by_hash(sealed_block.hash(), BlockSource::Any)?
        .ok_or(eyre::eyre!("block hash not found"))?;
    assert_eq!(block, block_by_hash3);
    Ok(())
}

/// The `ReceiptProvider` allows querying the receipts tables.
fn receipts_provider_example<
    T: ReceiptProvider<Receipt = reth_ethereum::Receipt>
        + TransactionsProvider<Transaction = TransactionSigned>
        + HeaderProvider,
>(
    provider: T,
) -> eyre::Result<()> {
    let txid = 5;
    let header_num = 100;

    // Query a receipt by txid
    let receipt = provider.receipt(txid)?.ok_or(eyre::eyre!("tx receipt not found"))?;

    // Can query receipt by txhash too
    let tx = provider.transaction_by_id(txid)?.unwrap();
    let receipt_by_hash = provider
        .receipt_by_hash(*tx.tx_hash())?
        .ok_or(eyre::eyre!("tx receipt by hash not found"))?;
    assert_eq!(receipt, receipt_by_hash);

    // Can query all the receipts in a block
    let _receipts = provider
        .receipts_by_block(100.into())?
        .ok_or(eyre::eyre!("no receipts found for block"))?;

    // Can check if a address/topic filter is present in a header, if it is we query the block and
    // receipts and do something with the data
    // 1. get the bloom from the header
    let header = provider.header_by_number(header_num)?.unwrap();
    let bloom = header.logs_bloom();

    // 2. Construct the address/topics filters
    // For a hypothetical address, we'll want to filter down for a specific indexed topic (e.g.
    // `from`).
    let addr = Address::random();
    let topic = B256::random();

    // TODO: Make it clearer how to choose between event_signature(topic0) (event name) and the
    // other 3 indexed topics. This API is a bit clunky and not obvious to use at the moment.
    let filter = Filter::new().address(addr).event_signature(topic);

    // 3. If the address & topics filters match do something. We use the outer check against the
    // bloom filter stored in the header to avoid having to query the receipts table when there
    // is no instance of any event that matches the filter in the header.
    if filter.matches_bloom(bloom) {
        let receipts = provider.receipt(header_num)?.ok_or(eyre::eyre!("receipt not found"))?;
        for log in &receipts.logs {
            if filter.matches(log) {
                // Do something with the log e.g. decode it.
                println!("Matching log found! {log:?}")
            }
        }
    }

    Ok(())
}

fn state_provider_example<T: StateProvider + AccountReader>(provider: T) -> eyre::Result<()> {
    let address = Address::random();
    let storage_key = B256::random();

    // Can get account / storage state with simple point queries
    let _account = provider.basic_account(&address)?;
    let _code = provider.account_code(&address)?;
    let _storage = provider.storage(address, storage_key)?;
    // TODO: unimplemented.
    // let _proof = provider.proof(address, &[])?;

    Ok(())
}
