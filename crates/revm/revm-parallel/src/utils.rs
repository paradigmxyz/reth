use crate::{
    read_inspector::ReadInspector,
    rw_set::{BlockRWSet, RevmAccessSet, TransitionRWSet},
};
use reth_interfaces::{
    executor::{BlockExecutionError, BlockValidationError},
    RethError, RethResult,
};
use reth_primitives::{
    revm::env::{fill_cfg_and_block_env, fill_tx_env},
    Address, Block, ChainSpec, Hardfork, U256,
};
use reth_revm_executor::state_change::post_block_balance_increments;
use revm::{db::State, DBBox, EVM};

/// Resolve block dependencies by executing it sequentially and recording reads and writes.
pub fn resolve_block_dependencies<'a>(
    state: &mut State<DBBox<'a, RethError>>,
    chain_spec: &ChainSpec,
    block: &Block,
    senders: &[Address],
    td: U256,
) -> RethResult<BlockRWSet> {
    if block.number == 0 {
        return Ok(BlockRWSet::default())
    }

    let mut block_rw_set = BlockRWSet::with_capacity(block.body.len());

    let mut evm = EVM::new();
    evm.database(state);

    fill_cfg_and_block_env(&mut evm.env.cfg, &mut evm.env.block, &chain_spec, &block.header, td);

    evm.db.as_mut().unwrap().set_state_clear_flag(
        chain_spec.fork(Hardfork::SpuriousDragon).active_at_block(block.number),
    );

    // TODO: apply beacon root call

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

        let mut rw_set = TransitionRWSet {
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

        block_rw_set.transactions.push(rw_set);
    }

    let post_block_increments = post_block_balance_increments(
        chain_spec,
        block.number,
        block.difficulty,
        block.beneficiary,
        block.timestamp,
        td,
        &block.ommers,
        block.withdrawals.as_deref(),
    );
    let mut post_block = TransitionRWSet::default();
    for (address, increment) in &post_block_increments {
        if *increment != 0 {
            post_block.write_set.account_balance(*address);
        }
    }
    block_rw_set.post_block = Some(post_block);
    evm.db.unwrap().increment_balances(post_block_increments)?;

    Ok(block_rw_set)
}
