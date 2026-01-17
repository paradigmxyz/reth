//! Transaction environment reuse utilities.
//!
//! This module provides traits and utilities for reusing [`TxEnv`] allocations
//! across multiple transaction executions, reducing per-transaction overhead.
//!
//! # Overview
//!
//! When executing many transactions (e.g., during block execution), the standard
//! pattern of creating a new [`TxEnv`] for each transaction incurs allocation
//! overhead for heap-allocated fields like `access_list`, `blob_hashes`, and
//! `authorization_list`.
//!
//! This module provides:
//! - [`FillTxEnv`]: A trait for filling an existing `TxEnv` in-place
//! - [`TxEnvExt`]: Extension methods for `TxEnv` including `clear_for_reuse`
//! - [`ReusableTxEnv`]: A wrapper for managing a reusable `TxEnv`
//!
//! # Example
//!
//! ```ignore
//! use reth_evm::tx_env::{ReusableTxEnv, FillTxEnv};
//!
//! let mut reusable = ReusableTxEnv::new();
//!
//! for tx in transactions {
//!     // Fill the reusable TxEnv with transaction data
//!     reusable.fill(tx.inner(), tx.signer());
//!     
//!     // Take ownership for EVM execution
//!     let tx_env = reusable.take();
//!     let result = evm.transact_raw(tx_env)?;
//!     
//!     // Get the TxEnv back from the EVM context for reuse
//!     // (implementation-specific, depends on EVM access)
//! }
//! ```

use alloy_consensus::{
    transaction::Recovered, EthereumTxEnvelope, Signed, TxEip1559, TxEip2930, TxEip4844,
    TxEip4844Variant, TxEip7702, TxLegacy,
};
use alloy_eips::{eip2718::WithEncoded, eip7702::RecoveredAuthorization, Typed2718};
use alloy_primitives::{Address, Bytes, TxKind};
use revm::{context::TxEnv, context_interface::either::Either};

/// A trait for filling an existing [`TxEnv`] in-place, avoiding allocation.
///
/// This is the in-place counterpart to [`alloy_evm::FromRecoveredTx`]. Instead of
/// creating a new `TxEnv`, it clears and fills an existing one while preserving
/// the capacity of heap-allocated fields like `access_list`, `blob_hashes`, and
/// `authorization_list`.
pub trait FillTxEnv<Tx> {
    /// Fills `self` with data from the transaction and sender address.
    ///
    /// This method should:
    /// 1. Clear all Vec fields using `clear()` to preserve capacity
    /// 2. Fill all fields with data from the transaction
    fn fill_from_recovered_tx(&mut self, tx: &Tx, sender: Address);
}

/// Extension trait for [`TxEnv`] to support reuse.
pub trait TxEnvExt {
    /// Clears the transaction environment while preserving allocated capacity.
    ///
    /// This is more efficient than creating a new `TxEnv` when executing many
    /// transactions because it avoids repeated allocations for Vec fields.
    fn clear_for_reuse(&mut self);
}

impl TxEnvExt for TxEnv {
    fn clear_for_reuse(&mut self) {
        self.access_list.0.clear();
        self.blob_hashes.clear();
        self.authorization_list.clear();
        self.data = Bytes::new();
    }
}

impl FillTxEnv<TxLegacy> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &TxLegacy, caller: Address) {
        self.clear_for_reuse();

        self.tx_type = tx.ty();
        self.caller = caller;
        self.gas_limit = tx.gas_limit;
        self.gas_price = tx.gas_price;
        self.kind = tx.to;
        self.value = tx.value;
        self.data = tx.input.clone();
        self.nonce = tx.nonce;
        self.chain_id = tx.chain_id;
        self.gas_priority_fee = None;
        self.max_fee_per_blob_gas = 0;
    }
}

impl FillTxEnv<Signed<TxLegacy>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &Signed<TxLegacy>, sender: Address) {
        self.fill_from_recovered_tx(tx.tx(), sender);
    }
}

impl FillTxEnv<TxEip2930> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &TxEip2930, caller: Address) {
        self.clear_for_reuse();

        self.tx_type = tx.ty();
        self.caller = caller;
        self.gas_limit = tx.gas_limit;
        self.gas_price = tx.gas_price;
        self.kind = tx.to;
        self.value = tx.value;
        self.data = tx.input.clone();
        self.nonce = tx.nonce;
        self.chain_id = Some(tx.chain_id);
        self.gas_priority_fee = None;
        self.max_fee_per_blob_gas = 0;

        self.access_list = tx.access_list.clone();
    }
}

impl FillTxEnv<Signed<TxEip2930>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &Signed<TxEip2930>, sender: Address) {
        self.fill_from_recovered_tx(tx.tx(), sender);
    }
}

impl FillTxEnv<TxEip1559> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &TxEip1559, caller: Address) {
        self.clear_for_reuse();

        self.tx_type = tx.ty();
        self.caller = caller;
        self.gas_limit = tx.gas_limit;
        self.gas_price = tx.max_fee_per_gas;
        self.kind = tx.to;
        self.value = tx.value;
        self.data = tx.input.clone();
        self.nonce = tx.nonce;
        self.chain_id = Some(tx.chain_id);
        self.gas_priority_fee = Some(tx.max_priority_fee_per_gas);
        self.max_fee_per_blob_gas = 0;

        self.access_list = tx.access_list.clone();
    }
}

impl FillTxEnv<Signed<TxEip1559>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &Signed<TxEip1559>, sender: Address) {
        self.fill_from_recovered_tx(tx.tx(), sender);
    }
}

impl FillTxEnv<TxEip4844> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &TxEip4844, caller: Address) {
        self.clear_for_reuse();

        self.tx_type = tx.ty();
        self.caller = caller;
        self.gas_limit = tx.gas_limit;
        self.gas_price = tx.max_fee_per_gas;
        self.kind = TxKind::Call(tx.to);
        self.value = tx.value;
        self.data = tx.input.clone();
        self.nonce = tx.nonce;
        self.chain_id = Some(tx.chain_id);
        self.gas_priority_fee = Some(tx.max_priority_fee_per_gas);
        self.max_fee_per_blob_gas = tx.max_fee_per_blob_gas;

        self.access_list = tx.access_list.clone();
        self.blob_hashes.extend_from_slice(&tx.blob_versioned_hashes);
    }
}

impl<T: AsRef<TxEip4844>> FillTxEnv<TxEip4844Variant<T>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &TxEip4844Variant<T>, caller: Address) {
        self.fill_from_recovered_tx(tx.tx().as_ref(), caller);
    }
}

impl<T: AsRef<TxEip4844>> FillTxEnv<Signed<TxEip4844Variant<T>>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &Signed<TxEip4844Variant<T>>, sender: Address) {
        self.fill_from_recovered_tx(tx.tx(), sender);
    }
}

impl FillTxEnv<TxEip7702> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &TxEip7702, caller: Address) {
        use alloy_consensus::crypto::secp256k1;
        use alloy_eips::eip7702::RecoveredAuthority;

        self.clear_for_reuse();

        self.tx_type = tx.ty();
        self.caller = caller;
        self.gas_limit = tx.gas_limit;
        self.gas_price = tx.max_fee_per_gas;
        self.kind = TxKind::Call(tx.to);
        self.value = tx.value;
        self.data = tx.input.clone();
        self.nonce = tx.nonce;
        self.chain_id = Some(tx.chain_id);
        self.gas_priority_fee = Some(tx.max_priority_fee_per_gas);
        self.max_fee_per_blob_gas = 0;

        self.access_list = tx.access_list.clone();

        self.authorization_list.extend(tx.authorization_list.iter().map(|auth| {
            Either::Right(RecoveredAuthorization::new_unchecked(
                auth.inner().clone(),
                auth.signature()
                    .ok()
                    .and_then(|signature| {
                        secp256k1::recover_signer(&signature, auth.signature_hash()).ok()
                    })
                    .map_or(RecoveredAuthority::Invalid, RecoveredAuthority::Valid),
            ))
        }));
    }
}

impl FillTxEnv<Signed<TxEip7702>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &Signed<TxEip7702>, sender: Address) {
        self.fill_from_recovered_tx(tx.tx(), sender);
    }
}

impl<Eip4844: AsRef<TxEip4844>> FillTxEnv<EthereumTxEnvelope<Eip4844>> for TxEnv {
    fn fill_from_recovered_tx(&mut self, tx: &EthereumTxEnvelope<Eip4844>, caller: Address) {
        match tx {
            EthereumTxEnvelope::Legacy(tx) => self.fill_from_recovered_tx(tx.tx(), caller),
            EthereumTxEnvelope::Eip1559(tx) => self.fill_from_recovered_tx(tx.tx(), caller),
            EthereumTxEnvelope::Eip2930(tx) => self.fill_from_recovered_tx(tx.tx(), caller),
            EthereumTxEnvelope::Eip4844(tx) => {
                self.fill_from_recovered_tx(tx.tx().as_ref(), caller)
            }
            EthereumTxEnvelope::Eip7702(tx) => self.fill_from_recovered_tx(tx.tx(), caller),
        }
    }
}

impl<T, TxEnvT> FillTxEnv<Recovered<T>> for TxEnvT
where
    TxEnvT: FillTxEnv<T>,
{
    fn fill_from_recovered_tx(&mut self, recovered: &Recovered<T>, _sender: Address) {
        self.fill_from_recovered_tx(recovered.inner(), recovered.signer());
    }
}

impl<T, TxEnvT> FillTxEnv<WithEncoded<Recovered<T>>> for TxEnvT
where
    TxEnvT: FillTxEnv<T>,
{
    fn fill_from_recovered_tx(
        &mut self,
        with_encoded: &WithEncoded<Recovered<T>>,
        _sender: Address,
    ) {
        let recovered = &with_encoded.1;
        self.fill_from_recovered_tx(recovered.inner(), recovered.signer());
    }
}

/// A wrapper that holds a reusable [`TxEnv`] for efficient transaction execution.
///
/// This struct is designed to be stored in block executors and reused across
/// multiple transaction executions within the same block, reducing allocation
/// overhead.
#[derive(Debug, Default)]
pub struct ReusableTxEnv {
    inner: TxEnv,
}

impl ReusableTxEnv {
    /// Takes the inner [`TxEnv`] by replacing it with a default value.
    ///
    /// This is useful for transferring ownership to an EVM that requires
    /// a `TxEnv` by value, while keeping this wrapper for future reuse.
    ///
    /// Note: After calling this, the inner `TxEnv` will be in a default state.
    /// Use [`Self::restore`] to put back a `TxEnv` for reuse.
    pub fn take(&mut self) -> TxEnv {
        core::mem::take(&mut self.inner)
    }

    /// Restores a [`TxEnv`] for reuse.
    ///
    /// This should be called after taking a `TxEnv` via [`Self::take`] and
    /// using it for execution. The restored `TxEnv` keeps its capacity for
    /// heap-allocated fields, making future fills more efficient.
    pub fn restore(&mut self, tx_env: TxEnv) {
        self.inner = tx_env;
    }
}

impl ReusableTxEnv {
    /// Creates a new [`ReusableTxEnv`] with default capacity.
    pub fn new() -> Self {
        Self { inner: TxEnv::default() }
    }

    /// Fills the inner [`TxEnv`] from a recovered transaction.
    ///
    /// This clears the existing data while preserving capacity, then fills
    /// with data from the transaction.
    pub fn fill<Tx>(&mut self, tx: &Tx, sender: Address)
    where
        TxEnv: FillTxEnv<Tx>,
    {
        self.inner.fill_from_recovered_tx(tx, sender);
    }

    /// Returns a reference to the inner [`TxEnv`].
    pub fn as_tx_env(&self) -> &TxEnv {
        &self.inner
    }

    /// Returns a mutable reference to the inner [`TxEnv`].
    pub fn as_tx_env_mut(&mut self) -> &mut TxEnv {
        &mut self.inner
    }

    /// Consumes self and returns the inner [`TxEnv`].
    pub fn into_inner(self) -> TxEnv {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, U256};

    #[test]
    fn test_fill_legacy_tx() {
        let mut tx_env = TxEnv::default();
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 42,
            gas_price: 100,
            gas_limit: 21000,
            to: TxKind::Call(address!("0x1234567890123456789012345678901234567890")),
            value: U256::from(1000),
            input: Bytes::from_static(b"hello"),
        };
        let sender = address!("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd");

        tx_env.fill_from_recovered_tx(&tx, sender);

        assert_eq!(tx_env.caller, sender);
        assert_eq!(tx_env.nonce, 42);
        assert_eq!(tx_env.gas_price, 100);
        assert_eq!(tx_env.gas_limit, 21000);
        assert_eq!(tx_env.value, U256::from(1000));
        assert_eq!(tx_env.chain_id, Some(1));
    }

    #[test]
    fn test_clear_preserves_capacity() {
        let mut tx_env = TxEnv::default();

        tx_env
            .blob_hashes
            .push(b256!("0x0102030405060708091011121314151617181920212223242526272829303132"));
        tx_env
            .blob_hashes
            .push(b256!("0x0102030405060708091011121314151617181920212223242526272829303133"));
        let capacity_before = tx_env.blob_hashes.capacity();

        tx_env.clear_for_reuse();

        assert!(tx_env.blob_hashes.is_empty());
        assert_eq!(tx_env.blob_hashes.capacity(), capacity_before);
    }

    #[test]
    fn test_reusable_tx_env() {
        let mut reusable = ReusableTxEnv::new();
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 1,
            gas_price: 100,
            gas_limit: 21000,
            to: TxKind::Call(address!("0x1234567890123456789012345678901234567890")),
            value: U256::from(100),
            input: Bytes::new(),
        };
        let sender = address!("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd");

        reusable.fill(&tx, sender);
        assert_eq!(reusable.as_tx_env().nonce, 1);

        let tx2 = TxLegacy { nonce: 2, ..tx.clone() };
        reusable.fill(&tx2, sender);
        assert_eq!(reusable.as_tx_env().nonce, 2);
    }
}
