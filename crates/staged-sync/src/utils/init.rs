use reth_db::{
    cursor::DbCursorRO,
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    constants::EIP1559_INITIAL_BASE_FEE, Account, ChainSpec, Hardfork, Header, H256,
};
use std::{path::Path, sync::Arc};
use tracing::debug;

/// Opens up an existing database or creates a new one at the specified path.
pub fn init_db<P: AsRef<Path>>(path: P) -> eyre::Result<Env<WriteMap>> {
    std::fs::create_dir_all(path.as_ref())?;
    let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
        path.as_ref(),
        reth_db::mdbx::EnvKind::RW,
    )?;
    db.create_tables()?;

    Ok(db)
}

/// Write the genesis block if it has not already been written
#[allow(clippy::field_reassign_with_default)]
pub fn init_genesis<DB: Database>(db: Arc<DB>, chain: ChainSpec) -> Result<H256, reth_db::Error> {
    let genesis = chain.genesis();
    let tx = db.tx()?;
    if let Some((_, hash)) = tx.cursor_read::<tables::CanonicalHeaders>()?.first()? {
        debug!("Genesis already written, skipping.");
        return Ok(hash)
    }

    drop(tx);
    debug!("Writing genesis block.");
    let tx = db.tx_mut()?;

    // Insert account state
    for (address, account) in &genesis.alloc {
        tx.put::<tables::PlainAccountState>(
            *address,
            Account {
                nonce: account.nonce.unwrap_or_default(),
                balance: account.balance,
                bytecode_hash: None,
            },
        )?;
    }

    // Insert header
    let mut header: Header = genesis.clone().into();

    // set base fee if EIP-1559 is enabled
    if chain.fork_active(Hardfork::London, 0) {
        header.base_fee_per_gas = Some(EIP1559_INITIAL_BASE_FEE);
    }

    let hash = header.hash_slow();
    tx.put::<tables::CanonicalHeaders>(0, hash)?;
    tx.put::<tables::HeaderNumbers>(hash, 0)?;
    tx.put::<tables::BlockBodies>((0, hash).into(), Default::default())?;
    tx.put::<tables::BlockTransitionIndex>(0, 0)?;
    tx.put::<tables::HeaderTD>((0, hash).into(), header.difficulty.into())?;
    tx.put::<tables::Headers>((0, hash).into(), header)?;

    tx.commit()?;
    Ok(hash)
}

#[cfg(test)]
mod tests {

    use super::init_genesis;
    use reth_db::mdbx::test_utils::create_test_rw_db;
    use reth_primitives::{
        GOERLI, GOERLI_GENESIS, MAINNET, MAINNET_GENESIS, SEPOLIA, SEPOLIA_GENESIS,
    };

    #[test]
    fn success_init_genesis_mainnet() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db.clone(), MAINNET.clone()).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, MAINNET_GENESIS);
    }

    #[test]
    fn success_init_genesis_goerli() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db.clone(), GOERLI.clone()).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, GOERLI_GENESIS);
    }

    #[test]
    fn success_init_genesis_sepolia() {
        let db = create_test_rw_db();
        let genesis_hash = init_genesis(db.clone(), SEPOLIA.clone()).unwrap();

        // actual, expected
        assert_eq!(genesis_hash, SEPOLIA_GENESIS);
    }
}
