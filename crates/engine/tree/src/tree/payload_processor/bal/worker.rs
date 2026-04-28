//! BAL-path block executor — a `BasicBlockExecutor`-shaped wrapper that runs transactions
//! against a snapshot-backed state with revm's `BalDatabase` layered on top.
//!
//! `BalBlockExecutor` owns the `Evm` config and the layered revm `State`, mirroring reth's
//! `BasicBlockExecutor` pattern (see `crates/evm/evm/src/execute.rs:529`). Methods on
//! `&mut self` hold both fields simultaneously, which lets revm's
//! `ConfigureEvm::create_executor` unify its three lifetimes (`&self`, `&mut State`,
//! `ExecutionCtx<'_>`) at the method boundary — the lifetime trick that made a standalone
//! generic helper infeasible.
//!
//! ### Error classification (limited)
//!
//! revm's `BalError` is a unit variant with no address/slot metadata, wrapped inside
//! `BlockExecutionError::Internal(InternalBlockExecutionError::EVM { error: Box<dyn Error>,
//! .. })`. Reliably distinguishing "undeclared access" from other EVM failures requires
//! downcasting through revm's `EVMError<DB::Error, TxError>`, which we defer until upstream
//! revm carries the richer error. For now all EVM failures surface as [`WorkerError::Evm`].
//! The main-thread commit loop (step 4c) still catches adversarial BALs via checks E and F.

use super::{pre_state::BlockPreState, snapshot_db::SnapshotDatabase};
use alloy_evm::block::{BlockExecutionError, BlockExecutor, TxResult};
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor, HaltReasonFor};
use revm::{context::result::ResultAndState, database::State};
use revm_state::bal::Bal;
use std::sync::Arc;

/// Errors emitted by a worker executing one transaction.
#[derive(Debug)]
pub enum WorkerError {
    /// EVM execution failed. Covers revm `BalError` (undeclared access), internal EVM errors,
    /// and transaction validation failures.
    // TODO: once revm's `BalError` carries address/slot metadata, split out a dedicated
    // `UndeclaredAccess` variant so the commit loop can emit `RejectReason::UndeclaredAccess`
    // without downcasting through `Box<dyn Error>`.
    Evm(BlockExecutionError),
}

impl core::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Evm(e) => write!(f, "evm execution failed: {e}"),
        }
    }
}

impl core::error::Error for WorkerError {}

/// A `BlockExecutor`-shaped worker that executes one transaction per call against a
/// snapshot-backed state with revm's `BalDatabase` layered on top.
///
/// Owns the `Evm` config (cloned from the engine's) and the layered `State<SnapshotDatabase>`.
/// The `&mut self` receiver on [`execute_tx`](Self::execute_tx) lets revm's `create_executor`
/// unify its lifetimes — the same pattern `BasicBlockExecutor::execute_one` uses.
#[expect(missing_debug_implementations)]
pub struct BalBlockExecutor<Evm: ConfigureEvm> {
    evm_config: Evm,
    state: State<SnapshotDatabase>,
}

impl<Evm: ConfigureEvm> BalBlockExecutor<Evm> {
    /// Builds a new worker. The layered revm `State` is constructed once and reused across
    /// `execute_tx` calls on this instance — suitable for sequential per-tx iteration on one
    /// thread. Each thread in a pool holds its own instance.
    pub fn new(evm_config: Evm, snapshot: Arc<BlockPreState>, received_bal: Arc<Bal>) -> Self {
        let state = State::builder()
            .with_database(SnapshotDatabase::new(snapshot))
            .with_bal(received_bal)
            .with_bundle_update()
            .build();
        Self { evm_config, state }
    }

    /// Executes one transaction. Returns its `ResultAndState` on success, or a
    /// [`WorkerError`] otherwise. Sets `state.bal_index = tx_index` so revm's `BalDatabase`
    /// resolves mid-block reads at the correct in-block position.
    ///
    /// The caller must ensure `tx_index` is 1-based (EIP-7928 reserves `0` for pre-exec and
    /// `n+1` for post-exec).
    pub fn execute_tx<'a, Tx>(
        &'a mut self,
        tx_index: u64,
        tx: Tx,
        evm_env: EvmEnvFor<Evm>,
        execution_ctx: ExecutionCtxFor<'a, Evm>,
    ) -> Result<ResultAndState<HaltReasonFor<Evm>>, WorkerError>
    where
        Tx: ExecutableTxFor<Evm>,
    {
        self.state.set_bal_index(tx_index);
        let evm = self.evm_config.evm_with_env(&mut self.state, evm_env);
        let mut executor = self.evm_config.create_executor(evm, execution_ctx);
        executor
            .execute_transaction_without_commit(tx)
            .map(TxResult::into_result)
            .map_err(WorkerError::Evm)
    }

    /// Consumes the worker and returns the underlying state. Useful once all txs are done
    /// and the caller wants the accumulated `BundleState` (e.g., for diagnostics).
    pub fn into_state(self) -> State<SnapshotDatabase> {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, U256};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::Account;
    use revm::Database;

    fn addr(byte: u8) -> Address {
        let mut a = [0u8; 20];
        a[19] = byte;
        Address::from(a)
    }

    /// BAL with given addresses declared so `BalDatabase` falls through to our snapshot.
    fn bal_declaring(addrs: &[Address]) -> Arc<Bal> {
        let mut bal = Bal::new();
        for a in addrs {
            bal.accounts.insert(*a, Default::default());
        }
        Arc::new(bal)
    }

    #[test]
    fn new_builds_state_that_reads_through_bal_to_snapshot() {
        // Verifies: the three-layer stack (EVM → State → BalDatabase → SnapshotDatabase) is
        // wired correctly. A declared address passes through the BAL layer and resolves from
        // our snapshot.
        let mut pre = BlockPreState::default();
        pre.accounts.insert(
            addr(1),
            Some(Account { balance: U256::from(42), nonce: 0, bytecode_hash: None }),
        );
        let snapshot = Arc::new(pre);

        let mut exec =
            BalBlockExecutor::new(EthEvmConfig::mainnet(), snapshot, bal_declaring(&[addr(1)]));

        let info = exec.state.basic(addr(1)).unwrap().unwrap();
        assert_eq!(info.balance, U256::from(42));
    }

    #[test]
    fn execute_tx_compiles_with_eth_evm_config() {
        // Type-check smoke test: `execute_tx` is callable with the full `EthEvmConfig`-bound
        // generic machinery (EvmEnvFor, ExecutionCtxFor, HaltReasonFor). Actual end-to-end
        // execution of a real signed tx belongs with step 4c's integration harness, where
        // the commit loop gives us real block data to construct SealedHeader +
        // NextBlockEnvAttributes + a properly-signed Recovered<Tx>.
        //
        // This reference proves the lifetimes on `execute_tx` resolve for a concrete Evm
        // type — the whole point of moving from the standalone generic function to the
        // struct-method form.
        fn _ty_check<Tx>(
            worker: &mut BalBlockExecutor<EthEvmConfig>,
            tx_index: u64,
            tx: Tx,
            evm_env: EvmEnvFor<EthEvmConfig>,
            ctx: ExecutionCtxFor<'_, EthEvmConfig>,
        ) -> Result<ResultAndState<HaltReasonFor<EthEvmConfig>>, WorkerError>
        where
            Tx: ExecutableTxFor<EthEvmConfig>,
        {
            worker.execute_tx(tx_index, tx, evm_env, ctx)
        }
        // Ensure the compiler actually realizes it.
        let _ = _ty_check::<
            reth_primitives_traits::Recovered<reth_ethereum_primitives::TransactionSigned>,
        >;
    }

    #[test]
    fn worker_error_displays() {
        let err = WorkerError::Evm(BlockExecutionError::msg("boom"));
        let s = format!("{err}");
        assert!(s.contains("evm execution failed"));
        assert!(s.contains("boom"));
    }
}
