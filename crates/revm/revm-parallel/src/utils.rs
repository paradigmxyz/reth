use crate::{
    read_inspector::ReadInspector,
    rw_set::{RevmAccessSet, TransactionRWSet},
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    RethError, RethResult,
};
use reth_primitives::{
    revm::env::{fill_cfg_and_block_env, fill_tx_env},
    Address, Block, ChainSpec, Hardfork, U256,
};
use revm::{db::State, DBBox, EVM};

/// Resolve block dependencies by executing it sequentially and recording reads and writes.
pub fn resolve_block_dependencies<'a>(
    chain_spec: &ChainSpec,
    database: DBBox<'a, RethError>,
    block: &Block,
    senders: &[Address],
    td: U256,
) -> RethResult<(Vec<TransactionRWSet>, State<DBBox<'a, RethError>>)> {
    let mut evm = EVM::new();
    evm.database(
        State::builder()
            .with_database_boxed(database)
            .with_bundle_update()
            .without_state_clear()
            .build(),
    );

    fill_cfg_and_block_env(&mut evm.env.cfg, &mut evm.env.block, &chain_spec, &block.header, td);

    evm.db.as_mut().unwrap().set_state_clear_flag(
        chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(block.number),
    );

    // TODO: apply beacon root call

    let mut tx_rw_sets = Vec::with_capacity(block.body.len());
    for (transaction, sender) in block.body.iter().zip(senders.iter()) {
        fill_tx_env(&mut evm.env.tx, transaction, *sender);

        let mut inspector = ReadInspector::default();
        let result = evm.inspect(&mut inspector).map_err(|error| {
            BlockExecutionError::Validation(BlockValidationError::EVM {
                hash: transaction.hash,
                error: error.into(),
            })
        })?;

        let evm_db = evm.db.as_mut().unwrap();

        let mut rw_set = TransactionRWSet {
            read_set: inspector.into_inner(),
            write_set: RevmAccessSet::default(),
        };

        // Record transaction `from` and `to` reads.
        // NOTE: The writes are covered by state transitions.
        rw_set.read_set.account_nonce(*sender);
        rw_set.read_set.account_balance(*sender);
        if let Some(to_address) = transaction.to() {
            rw_set.read_set.account_code(to_address);
        }

        let transitions = evm_db.cache.apply_evm_state(result.state);
        for (address, transition_account) in &transitions {
            rw_set.record_transition(*address, &transition_account);
        }
        evm_db.apply_transition(transitions);

        tx_rw_sets.push(rw_set);
    }

    Ok((tx_rw_sets, evm.db.unwrap()))
}
