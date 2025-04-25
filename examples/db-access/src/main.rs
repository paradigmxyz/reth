#![warn(unused_crate_dependencies)]

use reth_ethereum::{
    chainspec::ChainSpecBuilder,
    evm::revm::bytecode::opcode,
    node::EthereumNode,
    primitives::Bytecode,
    provider::{
        db::{cursor::DbCursorRO, tables, transaction::DbTx},
        providers::ReadOnlyConfig,
        DatabaseProviderFactory,
    },
    storage::DBProvider,
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
    let provider = factory.database_provider_ro()?;

    check_bytecodes(provider);

    Ok(())
}

fn check_bytecodes(provider: impl DBProvider) {
    let total_entries = provider.tx_ref().entries::<tables::Bytecodes>().unwrap();
    let mut cursor = provider.tx_ref().cursor_read::<tables::Bytecodes>().unwrap();

    let mut processed = 0;
    for entry in cursor.walk(None).unwrap() {
        let (hash, bytecode) = entry.unwrap();

        if check_bytecode(bytecode) {
            println!("Found bytecode: {hash:?}");
        }

        if processed % 1000 == 0 {
            println!("Processed {processed}/{total_entries}");
        }

        processed += 1;
    }
}

fn check_bytecode(bytecode: Bytecode) -> bool {
    let bytes = bytecode.0.bytecode();
    let mut idx: u8 = 0;
    let mut prev = None;

    while idx < bytes.len() as u8 {
        let opcode = bytes[idx as usize];

        if opcode >= opcode::PUSH1 && opcode <= opcode::PUSH32 {
            prev = None;
            idx += opcode - opcode::PUSH1 + 2;
            continue;
        }

        if opcode == opcode::JUMPDEST {
            if let Some(prev) = prev {
                if prev == 0xe6 || prev == 0xe7 || prev == 0xe8 {
                    return true;
                }
            }
        }

        prev = Some(opcode);
        idx += 1;
    }

    false
}
