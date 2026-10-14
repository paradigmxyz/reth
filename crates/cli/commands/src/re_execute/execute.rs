//! Shared local and corpus execution, consensus and historical-change verification.

use super::verify_bundle_against_changesets;
use alloy_consensus::{transaction::TxHashRef, BlockHeader, TxReceipt};
use alloy_eip7928::bal::Bal;
use eyre::WrapErr;
use reth_cli_util::cancellation::CancellationToken;
use reth_consensus::FullConsensus;
use reth_evm::{execute::Executor, ConfigureEvm, Database};
use reth_primitives_traits::{BlockBody, BlockTy, GotExpected, ReceiptTy, RecoveredBlock};
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader};
use std::{
    ops::Range,
    time::{Duration, Instant},
};
use tracing::error;

/// Execute an exclusive-end chunk. Each reset uses canonical poststate, never
/// execution output, and verifies the discarded bundle before continuing.
#[allow(clippy::too_many_arguments)]
pub(super) fn execute_chunk<E, D, P, C>(
    evm_config: &E,
    consensus: &C,
    provider: &P,
    range: Range<u64>,
    skip_invalid_blocks: bool,
    cancellation: &CancellationToken,
    mut db_at: impl FnMut(u64) -> eyre::Result<D>,
    mut block_at: impl FnMut(u64) -> eyre::Result<RecoveredBlock<BlockTy<E::Primitives>>>,
    mut receipts_at: impl FnMut(u64) -> eyre::Result<Vec<ReceiptTy<E::Primitives>>>,
    mut stats: impl FnMut(u64, u64),
    mut invalid: impl FnMut(RecoveredBlock<BlockTy<E::Primitives>>, eyre::Report),
) -> eyre::Result<()>
where
    E: ConfigureEvm,
    D: Database,
    P: ChangeSetReader + StorageChangeSetReader,
    C: FullConsensus<E::Primitives> + ?Sized,
{
    let mut executor = evm_config.batch_executor(db_at(range.start - 1)?);
    let mut executor_created = Instant::now();
    let mut bal_buf = Vec::new();
    'blocks: for number in range.clone() {
        if cancellation.is_cancelled() {
            eyre::bail!("Re-execution cancelled");
        }
        let block = block_at(number)?;
        let result = match executor.execute_one(&block) {
            Ok(result) => result,
            Err(err) => {
                if skip_invalid_blocks {
                    executor = evm_config.batch_executor(db_at(block.number())?);
                    invalid(block, eyre::Report::new(err));
                    continue;
                }
                return Err(err.into());
            }
        };
        let bal_hash =
            executor.take_bal().map(|bal| Bal::from(bal).compute_hash_with_buf(&mut bal_buf));
        if let Err(err) =
            consensus.validate_block_post_execution(&block, &result, None, bal_hash).wrap_err_with(
                || format!("Failed to validate block {} {}", block.number(), block.hash()),
            )
        {
            let correct_receipts = receipts_at(block.number())?;
            for (i, (receipt, correct_receipt)) in
                result.receipts.iter().zip(correct_receipts.iter()).enumerate()
            {
                if receipt != correct_receipt {
                    let tx_hash = block.body().transactions()[i].tx_hash();
                    error!(?receipt, ?correct_receipt, index = i, ?tx_hash, "Invalid receipt");
                    let expected_gas_used = correct_receipt.cumulative_gas_used() -
                        if i == 0 { 0 } else { correct_receipts[i - 1].cumulative_gas_used() };
                    let got_gas_used = receipt.cumulative_gas_used() -
                        if i == 0 { 0 } else { result.receipts[i - 1].cumulative_gas_used() };
                    if got_gas_used != expected_gas_used {
                        let mismatch =
                            GotExpected { expected: expected_gas_used, got: got_gas_used };
                        error!(number=?block.number(), ?mismatch, "Gas usage mismatch");
                        if skip_invalid_blocks {
                            executor = evm_config.batch_executor(db_at(block.number())?);
                            invalid(block, err);
                            continue 'blocks;
                        }
                        return Err(err);
                    }
                }
            }
            if skip_invalid_blocks {
                executor = evm_config.batch_executor(db_at(block.number())?);
                invalid(block, err);
                continue;
            }
            return Err(err);
        }
        stats(block.number(), block.gas_used());
        if executor.size_hint() > 5_000_000 || executor_created.elapsed() > Duration::from_secs(600)
        {
            let last_block = block.number();
            let old_executor =
                std::mem::replace(&mut executor, evm_config.batch_executor(db_at(last_block)?));
            verify_bundle_against_changesets(
                provider,
                &old_executor.into_state().take_bundle(),
                last_block,
            )?;
            executor_created = Instant::now();
        }
    }
    verify_bundle_against_changesets(provider, &executor.into_state().take_bundle(), range.end - 1)
}
