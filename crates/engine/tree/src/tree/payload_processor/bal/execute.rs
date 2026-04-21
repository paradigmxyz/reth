//! BAL-path block execution — workers produce per-tx results, a canonical executor commits
//! them in order while building the composed BAL.
//!
//! This is the main-thread commit loop from `BAL.md` §Main thread — canonical executor.
//!
//! ### The `Self::Result` type-punning
//!
//! Workers and the canonical executor are both built from the same `evm_config.create_executor`,
//! but worker's Evm is over [`SnapshotDatabase`] while canonical's is over the caller's `DB`.
//! `BlockExecutor::Result` depends on `Self::Evm`, so at the type level the two sides produce
//! *distinct* `Self::Result` types — even though for `EthEvmFactory` they concretely resolve
//! to the identical `EthTxResult<HaltReason, TxType>` (because [`EvmFactory::HaltReason`] is
//! factory-level, not DB-parameterized; see `alloy-evm-0.33.1/src/evm.rs:279`).
//!
//! Rust's type system can't express that parametricity as a `where`-clause, so threading the
//! worker output into `canonical.commit_transaction(worker_result)` requires a deliberate
//! type-pun. [`reinterpret_result`] is the single `unsafe` site where this happens. It is
//! sound **only** when the `EvmFactory`'s `HaltReason` and the receipt builder's transaction
//! type are DB-independent — which holds for `EthEvmFactory` (mainnet Ethereum) and is the
//! assumption under which this function operates.
//!
//! Future work (blocks removing the unsafe):
//! - Expose `BlockExecutorFactory::Result` as an associated type in alloy-evm so it's named on the
//!   factory rather than the executor, and parameterize it only by factory-level associated types.
//!   Then Rust can prove the two sides produce the same `Result`.
//!
//! ### Checks performed
//!
//! - **A (structural hash)**: `check_bal_hash(received_bal, header_bal_hash)` at entry.
//! - **B (item-count gate)**: `check_item_count(received_bal, block_gas_limit)` at entry.
//! - **D (feasibility)**: after each tx commits, via [`FeasibilityTracker`].
//! - **F (final hash)**: rebuilt composed BAL hashed against the header's commitment after
//!   post-execution.
//!
//! Check E (per-tx fragment compare) is skipped — check F is authoritative, and a
//! lightweight fragment compare isn't yet designed.
//!
//! ### Sequential; parallelism is future work
//!
//! One worker per tx on the calling thread. Workers' isolation from the canonical side is
//! preserved (separate states, separate `bal_builder`), so a future `WorkerPool` wrap around
//! the worker path would slot in without changing the commit-loop shape.

use super::{
    feasibility::FeasibilityTracker,
    pre_state::BlockPreState,
    snapshot_db::SnapshotDatabase,
    validation::{check_bal_hash, check_item_count},
    RejectReason,
};
use alloy_eip7928::{compute_block_access_list_hash, BlockAccessList};
use alloy_evm::{
    block::{BlockExecutionError, BlockExecutor, BlockExecutorFactory, TxResult},
    Evm,
};
use alloy_primitives::B256;
use reth_evm::{execute::ExecutableTxFor, ConfigureEvm};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use revm::{
    database::{states::bundle_state::BundleRetention, BundleState, State},
    Database,
};
use revm_state::bal::Bal;
use std::sync::Arc;

/// Alias for the canonical receipt type produced by a given `ConfigureEvm`. Factory-level
/// associated type — DB-independent.
pub type ReceiptFor<Evm> =
    <<Evm as ConfigureEvm>::BlockExecutorFactory as BlockExecutorFactory>::Receipt;

/// Output of a successful BAL-path block execution.
#[expect(missing_debug_implementations)]
pub struct BalExecutionOutput<Evm: ConfigureEvm> {
    /// Accumulated state transitions from the canonical executor.
    pub bundle_state: BundleState,
    /// Receipts produced in order, one per committed tx.
    pub receipts: Vec<ReceiptFor<Evm>>,
    /// Total gas used by all transactions.
    pub gas_used: u64,
    /// EIP-7685 withdrawal / deposit / consolidation requests.
    pub requests: alloy_eips::eip7685::Requests,
}

/// Errors surfaced by [`execute_block_bal`].
#[derive(Debug)]
pub enum BalExecutionError {
    /// BAL-specific rejection — structural, feasibility, or final-hash mismatch.
    Reject(RejectReason),
    /// Worker or canonical EVM failure (including revm `BalError` for undeclared accesses,
    /// surfaced opaquely until upstream revm carries address/slot metadata).
    Evm(BlockExecutionError),
    /// The received BAL could not be converted from alloy-format to revm-format.
    BalConversion(String),
}

impl From<RejectReason> for BalExecutionError {
    fn from(r: RejectReason) -> Self {
        Self::Reject(r)
    }
}

impl From<BlockExecutionError> for BalExecutionError {
    fn from(e: BlockExecutionError) -> Self {
        Self::Evm(e)
    }
}

impl core::fmt::Display for BalExecutionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Reject(r) => write!(f, "BAL rejection: {r:?}"),
            Self::Evm(e) => write!(f, "evm execution failed: {e}"),
            Self::BalConversion(s) => write!(f, "alloy→revm BAL conversion failed: {s}"),
        }
    }
}

impl core::error::Error for BalExecutionError {}

/// Top-level BAL-path block executor. Owns the `evm_config` so methods on `&self` give
/// revm's lifetime unification the method-scoped `&'a self` it needs.
#[expect(missing_debug_implementations)]
pub struct BalPayloadExecutor<Evm: ConfigureEvm> {
    evm_config: Evm,
}

impl<Evm: ConfigureEvm> BalPayloadExecutor<Evm> {
    /// Wraps an `evm_config`. Caller typically clones the engine's config.
    pub const fn new(evm_config: Evm) -> Self {
        Self { evm_config }
    }

    /// Executes one block on the BAL path.
    ///
    /// # Safety
    ///
    /// Contains one `unsafe` site (see module-level docs and [`reinterpret_result`]) that
    /// type-puns worker output into the canonical executor's `commit_transaction` input. The
    /// pun is sound only when the `EvmFactory`'s `HaltReason` is DB-independent, which holds
    /// for `EthEvmFactory`. Custom factories whose `HaltReason` varies by DB would produce
    /// unsound reads.
    #[expect(clippy::too_many_arguments)]
    pub fn execute_block<DB, Tx>(
        &self,
        canonical_db: DB,
        snapshot: Arc<BlockPreState>,
        received_bal: BlockAccessList,
        block: &SealedBlock<<Evm::Primitives as NodePrimitives>::Block>,
        txs: Vec<Tx>,
        header_bal_hash: B256,
        block_gas_limit: u64,
    ) -> Result<BalExecutionOutput<Evm>, BalExecutionError>
    where
        DB: Database + core::fmt::Debug,
        Tx: ExecutableTxFor<Evm>,
    {
        check_bal_hash(&received_bal, header_bal_hash)?;
        check_item_count(&received_bal, block_gas_limit)?;

        let received_bal_revm: Arc<Bal> = Arc::new(
            Bal::try_from(received_bal.clone())
                .map_err(|e| BalExecutionError::BalConversion(format!("{e:?}")))?,
        );

        let mut canonical_state = State::builder()
            .with_database(canonical_db)
            .with_bundle_update()
            .with_bal_builder()
            .build();

        // Pre-load every BAL-declared address into canonical state's cache. `State::commit`
        // (called by `commit_transaction`) panics at revm-database's
        // `cache.rs:195` ("All accounts should be present inside cache") when it tries to
        // apply a diff for an address not previously loaded. In the normal serial flow the
        // EVM loads the account itself during execution, but in option (a) the worker runs
        // the EVM on its own state and we only feed the diff to canonical — canonical never
        // read those accounts, so they aren't in its cache.
        for account_changes in &received_bal {
            canonical_state
                .load_cache_account(account_changes.address)
                .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;
        }

        // Declare `worker_state` at the outer scope so its lifetime matches canonical_state's
        // — both are method-local, and their borrows unify at the same `'a` when
        // `executor_for_block` is called. Each iteration re-initializes with a fresh `State`.
        //
        // The initial binding is immediately overwritten in the loop but gives us a valid
        // declaration site outside the loop body so the type is fixed once.
        let mut worker_state = State::builder()
            .with_database(SnapshotDatabase::new(snapshot.clone()))
            .with_bal(received_bal_revm.clone())
            .with_bundle_update()
            .build();
        let _ = &mut worker_state; // silence unused-assignment warning — we overwrite per iter

        let block_result = {
            let mut canonical_executor = self
                .evm_config
                .executor_for_block(&mut canonical_state, block)
                .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;

            canonical_executor.apply_pre_execution_changes()?;

            let mut feasibility = FeasibilityTracker::new(&received_bal, block_gas_limit);

            for (i, tx) in txs.into_iter().enumerate() {
                let tx_index = i as u64 + 1;

                // Reset worker state for the new tx (fresh snapshot view, zero bundle).
                worker_state = State::builder()
                    .with_database(SnapshotDatabase::new(snapshot.clone()))
                    .with_bal(received_bal_revm.clone())
                    .with_bundle_update()
                    .build();
                worker_state.set_bal_index(tx_index);

                let worker_result = {
                    let mut worker_executor = self
                        .evm_config
                        .executor_for_block(&mut worker_state, block)
                        .map_err(|e| BalExecutionError::Evm(BlockExecutionError::other(e)))?;

                    worker_executor.execute_transaction_without_commit(tx)?
                };

                feasibility.record_and_check(worker_result.result())?;

                canonical_executor.evm_mut().db_mut().bump_bal_index();
                // SAFETY: `worker_result` and `canonical_executor`'s `Self::Result` are
                // `EthTxResult<HaltReason, TxType>` for the same factory — bit-identical
                // even though the Evm types differ by DB. See module-level safety note.
                let canonical_result = unsafe { reinterpret_result(worker_result) };
                let _gas = canonical_executor.commit_transaction(canonical_result)?;
            }

            canonical_executor.evm_mut().db_mut().bump_bal_index();
            canonical_executor.apply_post_execution_changes()?
        };

        let composed_alloy = canonical_state.take_built_alloy_bal().expect("with_bal_builder set");
        let rebuilt = compute_block_access_list_hash(&composed_alloy);
        if rebuilt != header_bal_hash {
            return Err(BalExecutionError::Reject(RejectReason::FinalHashMismatch {
                rebuilt,
                expected: header_bal_hash,
            }));
        }

        canonical_state.merge_transitions(BundleRetention::Reverts);
        Ok(BalExecutionOutput {
            bundle_state: canonical_state.take_bundle(),
            receipts: block_result.receipts,
            gas_used: block_result.gas_used,
            requests: block_result.requests,
        })
    }
}

/// Reinterprets a worker's `Self::Result` as the canonical executor's `Self::Result`.
///
/// # Safety
///
/// Safe iff:
/// 1. The source and destination types are bit-identical.
/// 2. The source value is dropped via the destination type's `Drop` (uses `transmute_copy` +
///    `forget` to transfer ownership).
///
/// For `ConfigureEvm` implementations built atop `EthEvmFactory`, both `Self::Result` types
/// resolve to `EthTxResult<HaltReason, TxType>` regardless of DB, so (1) holds. (2) is
/// enforced by the function body. Non-Ethereum factories whose `HaltReason` depends on DB
/// would violate (1) — this function must not be called against them.
///
/// The runtime assertion is a defensive check; a size mismatch indicates that the calling
/// context does NOT satisfy (1) and the program would otherwise experience UB.
#[inline]
unsafe fn reinterpret_result<From, To>(src: From) -> To {
    assert_eq!(
        core::mem::size_of::<From>(),
        core::mem::size_of::<To>(),
        "BAL worker/canonical Self::Result types differ in size — \
         the factory's HaltReason is not DB-independent, which violates the safety invariant \
         of `execute_block_bal`. This probably means you're running against a custom \
         EvmFactory whose HaltReason varies with the database type.",
    );
    assert_eq!(
        core::mem::align_of::<From>(),
        core::mem::align_of::<To>(),
        "BAL worker/canonical Self::Result alignments differ — see size-mismatch message.",
    );
    // SAFETY: preconditions documented on the outer `unsafe fn`; size + align asserted above.
    let dst = unsafe { core::mem::transmute_copy(&src) };
    core::mem::forget(src);
    dst
}
