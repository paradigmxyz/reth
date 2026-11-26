//! A memory budget shared by the RPC requests that can hold a lot of memory.
//!
//! Tracing, proof, simulation and multi-call requests take units of the budget, the MiB their
//! parameters allow them to hold at most, until their work is done. A request that needs more than
//! the whole budget takes all of it and runs alone. A request waits at most [`MEMORY_BUDGET_WAIT`]
//! for its units and then fails with [`EthApiError::MemoryBudgetExhausted`], so that the caller can
//! retry elsewhere instead of queueing behind the work the node is doing.

use crate::EthApiError;
use alloy_rpc_types_eth::TransactionRequest;
use std::{num::NonZeroU32, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// How long a request waits for its share of the memory budget before it fails.
pub const MEMORY_BUDGET_WAIT: Duration = Duration::from_millis(200);

const MIB: u64 = 1024 * 1024;

/// Memory, in bytes, the first traced call of a request may hold besides what its tracer records:
/// its execution state and the response built from the trace.
const TRACE_CALL_BYTES: u64 = 16 * MIB;

/// Memory, in bytes, every further traced call of a request may hold besides what its tracer
/// records. The calls of a request run one after another and build one response, so only the first
/// needs [`TRACE_CALL_BYTES`].
const TRACE_NEXT_CALL_BYTES: u64 = 4 * MIB;

/// Memory, in bytes, every simulated block holds besides what its calls' gas pays for.
const SIMULATED_BLOCK_BYTES: u64 = MIB;

/// Memory, in bytes, every executed call holds besides what its gas pays for.
const CALL_BYTES: u64 = 64 * 1024;

/// The memory, in MiB, that the heavy RPC requests of a node may hold at once.
///
/// Clones share the same budget.
#[derive(Debug, Clone)]
pub struct RpcMemoryBudget {
    units: Arc<Semaphore>,
    total_mib: u32,
}

impl RpcMemoryBudget {
    /// Creates a budget of `total_mib` MiB.
    pub fn new(total_mib: NonZeroU32) -> Self {
        let total_mib = total_mib.get();
        Self { units: Arc::new(Semaphore::new(total_mib as usize)), total_mib }
    }

    /// Returns the size of the budget in MiB.
    pub const fn total_mib(&self) -> u32 {
        self.total_mib
    }

    /// Returns the MiB of the budget not taken by any request.
    pub fn available_mib(&self) -> usize {
        self.units.available_permits()
    }

    /// Takes `weight_mib` MiB of the budget, or all of it if the weight exceeds the budget.
    ///
    /// The permit holds the units until it is dropped. Returns
    /// [`EthApiError::MemoryBudgetExhausted`] if the units are not available within
    /// [`MEMORY_BUDGET_WAIT`].
    pub async fn acquire(&self, weight_mib: u64) -> Result<OwnedSemaphorePermit, EthApiError> {
        let units = u32::try_from(weight_mib).unwrap_or(u32::MAX).clamp(1, self.total_mib);
        match tokio::time::timeout(MEMORY_BUDGET_WAIT, self.units.clone().acquire_many_owned(units))
            .await
        {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(EthApiError::InternalEthError),
            Err(_) => Err(EthApiError::MemoryBudgetExhausted { weight_mib }),
        }
    }
}

/// Returns `bytes` in MiB, rounded up.
pub const fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(MIB)
}

/// Returns the memory, in bytes, that `calls` traced calls of one request may hold besides what
/// their tracers record.
pub const fn traced_calls_bytes(calls: u64) -> u64 {
    if calls == 0 {
        return 0
    }
    TRACE_CALL_BYTES.saturating_add((calls - 1).saturating_mul(TRACE_NEXT_CALL_BYTES))
}

/// Returns the most memory, in MiB, that executing `calls` calls with `gas` gas in total across
/// `blocks` simulated blocks may hold.
///
/// The state a call touches and the data it returns grow with its gas, which the calls share, so
/// the number of calls and blocks adds only what each keeps without gas: a block's header and
/// receipts, a call's result.
pub const fn execution_weight_mib(calls: u64, gas: u64, blocks: u64) -> u64 {
    bytes_to_mib(
        blocks
            .saturating_mul(SIMULATED_BLOCK_BYTES)
            .saturating_add(calls.saturating_mul(CALL_BYTES))
            .saturating_add(gas / 8),
    )
}

/// Returns the most memory, in MiB, that a proof of `slots` storage slots across `targets` accounts
/// may hold.
///
/// Each proven account or slot keeps the trie nodes on its path.
pub const fn proof_weight_mib(targets: u64, slots: u64) -> u64 {
    bytes_to_mib(4 * MIB + targets.saturating_add(slots).saturating_mul(8 * 1024))
}

/// Returns the gas `calls` may use in total: their own gas, or the RPC gas cap for those that
/// name none, at most the RPC gas cap the calls share. A `gas_cap` of 0 is no cap.
pub fn calls_gas<'a, T>(gas_cap: u64, calls: impl IntoIterator<Item = &'a T>) -> u64
where
    T: AsRef<TransactionRequest> + 'a,
{
    let cap = if gas_cap == 0 { u64::MAX } else { gas_cap };
    calls
        .into_iter()
        .map(|call| call.as_ref().gas.unwrap_or(cap))
        .fold(0u64, u64::saturating_add)
        .min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(total_mib: u32) -> RpcMemoryBudget {
        RpcMemoryBudget::new(NonZeroU32::new(total_mib).unwrap())
    }

    #[tokio::test(start_paused = true)]
    async fn units_return_when_permit_drops() {
        let budget = budget(100);
        let permit = budget.acquire(60).await.unwrap();
        assert_eq!(budget.available_mib(), 40);
        drop(permit);
        assert_eq!(budget.available_mib(), 100);
    }

    #[tokio::test(start_paused = true)]
    async fn fails_when_units_stay_taken() {
        let budget = budget(100);
        let _permit = budget.acquire(60).await.unwrap();
        let err = budget.acquire(50).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "request needs 50 MiB of the RPC memory budget, which is in use"
        );
        // The failed request takes nothing.
        assert_eq!(budget.available_mib(), 40);
        assert!(budget.acquire(40).await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn waits_for_units_within_the_wait() {
        let budget = budget(100);
        let permit = budget.acquire(100).await.unwrap();
        let waiting = tokio::spawn({
            let budget = budget.clone();
            async move { budget.acquire(10).await.map(drop) }
        });
        tokio::time::sleep(MEMORY_BUDGET_WAIT / 2).await;
        drop(permit);
        waiting.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn oversized_request_takes_whole_budget() {
        let budget = budget(100);
        let permit = budget.acquire(u64::MAX).await.unwrap();
        assert_eq!(budget.available_mib(), 0);
        assert_eq!(
            budget.acquire(1).await.unwrap_err().to_string(),
            "request needs 1 MiB of the RPC memory budget, which is in use"
        );
        drop(permit);
        assert_eq!(budget.available_mib(), 100);
    }

    #[test]
    fn calls_share_gas_cap() {
        let call = |gas: Option<u64>| TransactionRequest { gas, ..Default::default() };
        assert_eq!(calls_gas(100, &[call(Some(30)), call(Some(40))]), 70);
        assert_eq!(calls_gas(100, &[call(Some(30)), call(None)]), 100);
        assert_eq!(calls_gas(0, &[call(Some(30)), call(Some(40))]), 70);
        assert_eq!(calls_gas(0, &[call(None)]), u64::MAX);
    }

    #[test]
    fn weights() {
        assert_eq!(execution_weight_mib(1, 0, 0), 1);
        assert_eq!(execution_weight_mib(1, 80 * MIB, 0), 11);
        assert_eq!(execution_weight_mib(4, 0, 2), 3);
        // Many calls or blocks without gas stay well below a typical budget.
        assert_eq!(execution_weight_mib(1024, 0, 0), 64);
        assert_eq!(execution_weight_mib(1, 0, 256), 257);
        assert_eq!(proof_weight_mib(1, 1024), 13);
        assert_eq!(traced_calls_bytes(0), 0);
        assert_eq!(traced_calls_bytes(1), 16 * MIB);
        assert_eq!(traced_calls_bytes(64), 268 * MIB);
        assert_eq!(execution_weight_mib(u64::MAX, u64::MAX, u64::MAX), bytes_to_mib(u64::MAX));
    }
}
