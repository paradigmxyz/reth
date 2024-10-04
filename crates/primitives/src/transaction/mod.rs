//! Transaction types.

use crate::BlockHashOrNumber;
use alloy_eips::eip7702::SignedAuthorization;
use alloy_primitives::{keccak256, Address, TxKind, B256, U256};

use alloy_consensus::{SignableTransaction, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718},
    eip2930::AccessList,
};
use alloy_primitives::{Bytes, TxHash};
use alloy_rlp::{Decodable, Encodable, Error as RlpError, Header};
use core::mem;
use derive_more::{AsRef, Deref};
use once_cell::sync::Lazy;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use signature::{decode_with_eip155_chain_id, with_eip155_parity};

pub use error::{
    InvalidTransactionError, TransactionConversionError, TryFromRecoveredTransactionError,
};
pub use meta::TransactionMeta;
pub use pooled::{PooledTransactionsElement, PooledTransactionsElementEcRecovered};
#[cfg(all(feature = "c-kzg", any(test, feature = "arbitrary")))]
pub use sidecar::generate_blob_sidecar;
#[cfg(feature = "c-kzg")]
pub use sidecar::BlobTransactionValidationError;
pub use sidecar::{BlobTransaction, BlobTransactionSidecar};

pub use compat::FillTxEnv;
pub use signature::{
    extract_chain_id, legacy_parity, recover_signer, recover_signer_unchecked, Signature,
};
pub use tx_type::{
    TxType, EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};
pub use variant::TransactionSignedVariant;

pub(crate) mod access_list;
mod compat;
mod error;
mod meta;
mod pooled;
mod sidecar;
mod signature;
mod tx_type;
pub(crate) mod util;
mod variant;

#[cfg(feature = "optimism")]
use op_alloy_consensus::TxDeposit;
#[cfg(feature = "optimism")]
use reth_optimism_chainspec::optimism_deposit_tx_signature;
#[cfg(feature = "optimism")]
pub use tx_type::DEPOSIT_TX_TYPE_ID;
#[cfg(any(test, feature = "reth-codec"))]
use tx_type::{
    COMPACT_EXTENDED_IDENTIFIER_FLAG, COMPACT_IDENTIFIER_EIP1559, COMPACT_IDENTIFIER_EIP2930,
    COMPACT_IDENTIFIER_LEGACY,
};

#[cfg(test)]
use reth_codecs::Compact;

use alloc::vec::Vec;

/// Either a transaction hash or number.
pub type TxHashOrNumber = BlockHashOrNumber;

// Expected number of transactions where we can expect a speed-up by recovering the senders in
// parallel.
pub(crate) static PARALLEL_SENDER_RECOVERY_THRESHOLD: Lazy<usize> =
    Lazy::new(|| match rayon::current_num_threads() {
        0..=1 => usize::MAX,
        2..=8 => 10,
        _ => 5,
    });

/// A raw transaction.
///
/// Transaction types were introduced in [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, derive_more::From)]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub enum Transaction {
    /// Legacy transaction (type `0x0`).
    ///
    /// Traditional Ethereum transactions, containing parameters `nonce`, `gasPrice`, `gasLimit`,
    /// `to`, `value`, `data`, `v`, `r`, and `s`.
    ///
    /// These transactions do not utilize access lists nor do they incorporate EIP-1559 fee market
    /// changes.
    Legacy(TxLegacy),
    /// Transaction with an [`AccessList`] ([EIP-2930](https://eips.ethereum.org/EIPS/eip-2930)), type `0x1`.
    ///
    /// The `accessList` specifies an array of addresses and storage keys that the transaction
    /// plans to access, enabling gas savings on cross-contract calls by pre-declaring the accessed
    /// contract and storage slots.
    Eip2930(TxEip2930),
    /// A transaction with a priority fee ([EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)), type `0x2`.
    ///
    /// Unlike traditional transactions, EIP-1559 transactions use an in-protocol, dynamically
    /// changing base fee per gas, adjusted at each block to manage network congestion.
    ///
    /// - `maxPriorityFeePerGas`, specifying the maximum fee above the base fee the sender is
    ///   willing to pay
    /// - `maxFeePerGas`, setting the maximum total fee the sender is willing to pay.
    ///
    /// The base fee is burned, while the priority fee is paid to the miner who includes the
    /// transaction, incentivizing miners to include transactions with higher priority fees per
    /// gas.
    Eip1559(TxEip1559),
    /// Shard Blob Transactions ([EIP-4844](https://eips.ethereum.org/EIPS/eip-4844)), type `0x3`.
    ///
    /// Shard Blob Transactions introduce a new transaction type called a blob-carrying transaction
    /// to reduce gas costs. These transactions are similar to regular Ethereum transactions but
    /// include additional data called a blob.
    ///
    /// Blobs are larger (~125 kB) and cheaper than the current calldata, providing an immutable
    /// and read-only memory for storing transaction data.
    ///
    /// EIP-4844, also known as proto-danksharding, implements the framework and logic of
    /// danksharding, introducing new transaction formats and verification rules.
    Eip4844(TxEip4844),
    /// EOA Set Code Transactions ([EIP-7702](https://eips.ethereum.org/EIPS/eip-7702)), type `0x4`.
    ///
    /// EOA Set Code Transactions give the ability to temporarily set contract code for an
    /// EOA for a single transaction. This allows for temporarily adding smart contract
    /// functionality to the EOA.
    Eip7702(TxEip7702),
    /// Optimism deposit transaction.
    #[cfg(feature = "optimism")]
    Deposit(TxDeposit),
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Transaction {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let mut tx = match TxType::arbitrary(u)? {
            TxType::Legacy => {
                let tx = TxLegacy::arbitrary(u)?;
                Self::Legacy(tx)
            }
            TxType::Eip2930 => {
                let tx = TxEip2930::arbitrary(u)?;
                Self::Eip2930(tx)
            }
            TxType::Eip1559 => {
                let tx = TxEip1559::arbitrary(u)?;
                Self::Eip1559(tx)
            }
            TxType::Eip4844 => {
                let tx = TxEip4844::arbitrary(u)?;
                Self::Eip4844(tx)
            }

            TxType::Eip7702 => {
                let tx = TxEip7702::arbitrary(u)?;
                Self::Eip7702(tx)
            }
            #[cfg(feature = "optimism")]
            TxType::Deposit => {
                let tx = TxDeposit::arbitrary(u)?;
                Self::Deposit(tx)
            }
        };

        // Otherwise we might overflow when calculating `v` on `recalculate_hash`
        if let Some(chain_id) = tx.chain_id() {
            tx.set_chain_id(chain_id % (u64::MAX / 2 - 36));
        }

        Ok(tx)
    }
}

// === impl Transaction ===

impl Transaction {
    /// Heavy operation that return signature hash over rlp encoded transaction.
    /// It is only for signature signing or signer recovery.
    pub fn signature_hash(&self) -> B256 {
        match self {
            Self::Legacy(tx) => tx.signature_hash(),
            Self::Eip2930(tx) => tx.signature_hash(),
            Self::Eip1559(tx) => tx.signature_hash(),
            Self::Eip4844(tx) => tx.signature_hash(),
            Self::Eip7702(tx) => tx.signature_hash(),
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => B256::ZERO,
        }
    }

    /// Get `chain_id`.
    pub const fn chain_id(&self) -> Option<u64> {
        match self {
            Self::Legacy(TxLegacy { chain_id, .. }) => *chain_id,
            Self::Eip2930(TxEip2930 { chain_id, .. }) |
            Self::Eip1559(TxEip1559 { chain_id, .. }) |
            Self::Eip4844(TxEip4844 { chain_id, .. }) |
            Self::Eip7702(TxEip7702 { chain_id, .. }) => Some(*chain_id),
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => None,
        }
    }

    /// Sets the transaction's chain id to the provided value.
    pub fn set_chain_id(&mut self, chain_id: u64) {
        match self {
            Self::Legacy(TxLegacy { chain_id: ref mut c, .. }) => *c = Some(chain_id),
            Self::Eip2930(TxEip2930 { chain_id: ref mut c, .. }) |
            Self::Eip1559(TxEip1559 { chain_id: ref mut c, .. }) |
            Self::Eip4844(TxEip4844 { chain_id: ref mut c, .. }) |
            Self::Eip7702(TxEip7702 { chain_id: ref mut c, .. }) => *c = chain_id,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => { /* noop */ }
        }
    }

    /// Gets the transaction's [`TxKind`], which is the address of the recipient or
    /// [`TxKind::Create`] if the transaction is a contract creation.
    pub const fn kind(&self) -> TxKind {
        match self {
            Self::Legacy(TxLegacy { to, .. }) |
            Self::Eip2930(TxEip2930 { to, .. }) |
            Self::Eip1559(TxEip1559 { to, .. }) => *to,
            Self::Eip4844(TxEip4844 { to, .. }) | Self::Eip7702(TxEip7702 { to, .. }) => {
                TxKind::Call(*to)
            }
            #[cfg(feature = "optimism")]
            Self::Deposit(TxDeposit { to, .. }) => *to,
        }
    }

    /// Get the transaction's address of the contract that will be called, or the address that will
    /// receive the transfer.
    ///
    /// Returns `None` if this is a `CREATE` transaction.
    pub fn to(&self) -> Option<Address> {
        self.kind().to().copied()
    }

    /// Get the transaction's type
    pub const fn tx_type(&self) -> TxType {
        match self {
            Self::Legacy(_) => TxType::Legacy,
            Self::Eip2930(_) => TxType::Eip2930,
            Self::Eip1559(_) => TxType::Eip1559,
            Self::Eip4844(_) => TxType::Eip4844,
            Self::Eip7702(_) => TxType::Eip7702,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => TxType::Deposit,
        }
    }

    /// Gets the transaction's value field.
    pub const fn value(&self) -> U256 {
        *match self {
            Self::Legacy(TxLegacy { value, .. }) |
            Self::Eip2930(TxEip2930 { value, .. }) |
            Self::Eip1559(TxEip1559 { value, .. }) |
            Self::Eip4844(TxEip4844 { value, .. }) |
            Self::Eip7702(TxEip7702 { value, .. }) => value,
            #[cfg(feature = "optimism")]
            Self::Deposit(TxDeposit { value, .. }) => value,
        }
    }

    /// Get the transaction's nonce.
    pub const fn nonce(&self) -> u64 {
        match self {
            Self::Legacy(TxLegacy { nonce, .. }) |
            Self::Eip2930(TxEip2930 { nonce, .. }) |
            Self::Eip1559(TxEip1559 { nonce, .. }) |
            Self::Eip4844(TxEip4844 { nonce, .. }) |
            Self::Eip7702(TxEip7702 { nonce, .. }) => *nonce,
            // Deposit transactions do not have nonces.
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => 0,
        }
    }

    /// Returns the [`AccessList`] of the transaction.
    ///
    /// Returns `None` for legacy transactions.
    pub const fn access_list(&self) -> Option<&AccessList> {
        match self {
            Self::Legacy(_) => None,
            Self::Eip2930(tx) => Some(&tx.access_list),
            Self::Eip1559(tx) => Some(&tx.access_list),
            Self::Eip4844(tx) => Some(&tx.access_list),
            Self::Eip7702(tx) => Some(&tx.access_list),
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => None,
        }
    }

    /// Returns the [`SignedAuthorization`] list of the transaction.
    ///
    /// Returns `None` if this transaction is not EIP-7702.
    pub fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        match self {
            Self::Eip7702(tx) => Some(&tx.authorization_list),
            _ => None,
        }
    }

    /// Get the gas limit of the transaction.
    pub const fn gas_limit(&self) -> u64 {
        match self {
            Self::Legacy(TxLegacy { gas_limit, .. }) |
            Self::Eip1559(TxEip1559 { gas_limit, .. }) |
            Self::Eip4844(TxEip4844 { gas_limit, .. }) |
            Self::Eip7702(TxEip7702 { gas_limit, .. }) |
            Self::Eip2930(TxEip2930 { gas_limit, .. }) => *gas_limit,
            #[cfg(feature = "optimism")]
            Self::Deposit(TxDeposit { gas_limit, .. }) => *gas_limit,
        }
    }

    /// Returns true if the tx supports dynamic fees
    pub const fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::Legacy(_) | Self::Eip2930(_) => false,
            Self::Eip1559(_) | Self::Eip4844(_) | Self::Eip7702(_) => true,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => false,
        }
    }

    /// Max fee per gas for eip1559 transaction, for legacy transactions this is `gas_price`.
    ///
    /// This is also commonly referred to as the "Gas Fee Cap" (`GasFeeCap`).
    pub const fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Legacy(TxLegacy { gas_price, .. }) |
            Self::Eip2930(TxEip2930 { gas_price, .. }) => *gas_price,
            Self::Eip1559(TxEip1559 { max_fee_per_gas, .. }) |
            Self::Eip4844(TxEip4844 { max_fee_per_gas, .. }) |
            Self::Eip7702(TxEip7702 { max_fee_per_gas, .. }) => *max_fee_per_gas,
            // Deposit transactions buy their L2 gas on L1 and, as such, the L2 gas is not
            // refundable.
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => 0,
        }
    }

    /// Max priority fee per gas for eip1559 transaction, for legacy and eip2930 transactions this
    /// is `None`
    ///
    /// This is also commonly referred to as the "Gas Tip Cap" (`GasTipCap`).
    pub const fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Legacy(_) | Self::Eip2930(_) => None,
            Self::Eip1559(TxEip1559 { max_priority_fee_per_gas, .. }) |
            Self::Eip4844(TxEip4844 { max_priority_fee_per_gas, .. }) |
            Self::Eip7702(TxEip7702 { max_priority_fee_per_gas, .. }) => {
                Some(*max_priority_fee_per_gas)
            }
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => None,
        }
    }

    /// Blob versioned hashes for eip4844 transaction, for legacy, eip1559, eip2930 and eip7702
    /// transactions this is `None`
    ///
    /// This is also commonly referred to as the "blob versioned hashes" (`BlobVersionedHashes`).
    pub fn blob_versioned_hashes(&self) -> Option<Vec<B256>> {
        match self {
            Self::Legacy(_) | Self::Eip2930(_) | Self::Eip1559(_) | Self::Eip7702(_) => None,
            Self::Eip4844(TxEip4844 { blob_versioned_hashes, .. }) => {
                Some(blob_versioned_hashes.clone())
            }
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => None,
        }
    }

    /// Max fee per blob gas for eip4844 transaction [`TxEip4844`].
    ///
    /// Returns `None` for non-eip4844 transactions.
    ///
    /// This is also commonly referred to as the "Blob Gas Fee Cap" (`BlobGasFeeCap`).
    pub const fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::Eip4844(TxEip4844 { max_fee_per_blob_gas, .. }) => Some(*max_fee_per_blob_gas),
            _ => None,
        }
    }

    /// Returns the blob gas used for all blobs of the EIP-4844 transaction if it is an EIP-4844
    /// transaction.
    ///
    /// This is the number of blobs times the
    /// [`DATA_GAS_PER_BLOB`](crate::constants::eip4844::DATA_GAS_PER_BLOB) a single blob consumes.
    pub fn blob_gas_used(&self) -> Option<u64> {
        self.as_eip4844().map(TxEip4844::blob_gas)
    }

    /// Return the max priority fee per gas if the transaction is an EIP-1559 transaction, and
    /// otherwise return the gas price.
    ///
    /// # Warning
    ///
    /// This is different than the `max_priority_fee_per_gas` method, which returns `None` for
    /// non-EIP-1559 transactions.
    pub const fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::Legacy(TxLegacy { gas_price, .. }) |
            Self::Eip2930(TxEip2930 { gas_price, .. }) => *gas_price,
            Self::Eip1559(TxEip1559 { max_priority_fee_per_gas, .. }) |
            Self::Eip4844(TxEip4844 { max_priority_fee_per_gas, .. }) |
            Self::Eip7702(TxEip7702 { max_priority_fee_per_gas, .. }) => *max_priority_fee_per_gas,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => 0,
        }
    }

    /// Returns the effective gas price for the given base fee.
    ///
    /// If the transaction is a legacy or EIP2930 transaction, the gas price is returned.
    pub const fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match self {
            Self::Legacy(tx) => tx.gas_price,
            Self::Eip2930(tx) => tx.gas_price,
            Self::Eip1559(dynamic_tx) => dynamic_tx.effective_gas_price(base_fee),
            Self::Eip4844(dynamic_tx) => dynamic_tx.effective_gas_price(base_fee),
            Self::Eip7702(dynamic_tx) => dynamic_tx.effective_gas_price(base_fee),
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => 0,
        }
    }

    /// Returns the effective miner gas tip cap (`gasTipCap`) for the given base fee:
    /// `min(maxFeePerGas - baseFee, maxPriorityFeePerGas)`
    ///
    /// If the base fee is `None`, the `max_priority_fee_per_gas`, or gas price for non-EIP1559
    /// transactions is returned.
    ///
    /// Returns `None` if the basefee is higher than the [`Transaction::max_fee_per_gas`].
    pub fn effective_tip_per_gas(&self, base_fee: Option<u64>) -> Option<u128> {
        let base_fee = match base_fee {
            Some(base_fee) => base_fee as u128,
            None => return Some(self.priority_fee_or_price()),
        };

        let max_fee_per_gas = self.max_fee_per_gas();

        // Check if max_fee_per_gas is less than base_fee
        if max_fee_per_gas < base_fee {
            return None
        }

        // Calculate the difference between max_fee_per_gas and base_fee
        let fee = max_fee_per_gas - base_fee;

        // Compare the fee with max_priority_fee_per_gas (or gas price for non-EIP1559 transactions)
        if let Some(priority_fee) = self.max_priority_fee_per_gas() {
            Some(fee.min(priority_fee))
        } else {
            Some(fee)
        }
    }

    /// Get the transaction's input field.
    pub const fn input(&self) -> &Bytes {
        match self {
            Self::Legacy(TxLegacy { input, .. }) |
            Self::Eip2930(TxEip2930 { input, .. }) |
            Self::Eip1559(TxEip1559 { input, .. }) |
            Self::Eip4844(TxEip4844 { input, .. }) |
            Self::Eip7702(TxEip7702 { input, .. }) => input,
            #[cfg(feature = "optimism")]
            Self::Deposit(TxDeposit { input, .. }) => input,
        }
    }

    /// Returns the source hash of the transaction, which uniquely identifies its source.
    /// If not a deposit transaction, this will always return `None`.
    #[cfg(feature = "optimism")]
    pub const fn source_hash(&self) -> Option<B256> {
        match self {
            Self::Deposit(TxDeposit { source_hash, .. }) => Some(*source_hash),
            _ => None,
        }
    }

    /// Returns the amount of ETH locked up on L1 that will be minted on L2. If the transaction
    /// is not a deposit transaction, this will always return `None`.
    #[cfg(feature = "optimism")]
    pub const fn mint(&self) -> Option<u128> {
        match self {
            Self::Deposit(TxDeposit { mint, .. }) => *mint,
            _ => None,
        }
    }

    /// Returns whether or not the transaction is a system transaction. If the transaction
    /// is not a deposit transaction, this will always return `false`.
    #[cfg(feature = "optimism")]
    pub const fn is_system_transaction(&self) -> bool {
        match self {
            Self::Deposit(TxDeposit { is_system_transaction, .. }) => *is_system_transaction,
            _ => false,
        }
    }

    /// Returns whether or not the transaction is an Optimism Deposited transaction.
    #[cfg(feature = "optimism")]
    pub const fn is_deposit(&self) -> bool {
        matches!(self, Self::Deposit(_))
    }

    /// This encodes the transaction _without_ the signature, and is only suitable for creating a
    /// hash intended for signing.
    pub fn encode_without_signature(&self, out: &mut dyn bytes::BufMut) {
        Encodable::encode(self, out);
    }

    /// Inner encoding function that is used for both rlp [`Encodable`] trait and for calculating
    /// hash that for eip2718 does not require rlp header
    pub fn encode_with_signature(
        &self,
        signature: &Signature,
        out: &mut dyn bytes::BufMut,
        with_header: bool,
    ) {
        match self {
            Self::Legacy(legacy_tx) => {
                // do nothing w/ with_header
                legacy_tx.encode_with_signature_fields(
                    &with_eip155_parity(signature, legacy_tx.chain_id),
                    out,
                )
            }
            Self::Eip2930(access_list_tx) => {
                access_list_tx.encode_with_signature(signature, out, with_header)
            }
            Self::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encode_with_signature(signature, out, with_header)
            }
            Self::Eip4844(blob_tx) => blob_tx.encode_with_signature(signature, out, with_header),
            Self::Eip7702(set_code_tx) => {
                set_code_tx.encode_with_signature(signature, out, with_header)
            }
            #[cfg(feature = "optimism")]
            Self::Deposit(deposit_tx) => deposit_tx.encode_inner(out, with_header),
        }
    }

    /// This sets the transaction's gas limit.
    pub fn set_gas_limit(&mut self, gas_limit: u64) {
        match self {
            Self::Legacy(tx) => tx.gas_limit = gas_limit,
            Self::Eip2930(tx) => tx.gas_limit = gas_limit,
            Self::Eip1559(tx) => tx.gas_limit = gas_limit,
            Self::Eip4844(tx) => tx.gas_limit = gas_limit,
            Self::Eip7702(tx) => tx.gas_limit = gas_limit,
            #[cfg(feature = "optimism")]
            Self::Deposit(tx) => tx.gas_limit = gas_limit,
        }
    }

    /// This sets the transaction's nonce.
    pub fn set_nonce(&mut self, nonce: u64) {
        match self {
            Self::Legacy(tx) => tx.nonce = nonce,
            Self::Eip2930(tx) => tx.nonce = nonce,
            Self::Eip1559(tx) => tx.nonce = nonce,
            Self::Eip4844(tx) => tx.nonce = nonce,
            Self::Eip7702(tx) => tx.nonce = nonce,
            #[cfg(feature = "optimism")]
            Self::Deposit(_) => { /* noop */ }
        }
    }

    /// This sets the transaction's value.
    pub fn set_value(&mut self, value: U256) {
        match self {
            Self::Legacy(tx) => tx.value = value,
            Self::Eip2930(tx) => tx.value = value,
            Self::Eip1559(tx) => tx.value = value,
            Self::Eip4844(tx) => tx.value = value,
            Self::Eip7702(tx) => tx.value = value,
            #[cfg(feature = "optimism")]
            Self::Deposit(tx) => tx.value = value,
        }
    }

    /// This sets the transaction's input field.
    pub fn set_input(&mut self, input: Bytes) {
        match self {
            Self::Legacy(tx) => tx.input = input,
            Self::Eip2930(tx) => tx.input = input,
            Self::Eip1559(tx) => tx.input = input,
            Self::Eip4844(tx) => tx.input = input,
            Self::Eip7702(tx) => tx.input = input,
            #[cfg(feature = "optimism")]
            Self::Deposit(tx) => tx.input = input,
        }
    }

    /// Calculates a heuristic for the in-memory size of the [Transaction].
    #[inline]
    pub fn size(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.size(),
            Self::Eip2930(tx) => tx.size(),
            Self::Eip1559(tx) => tx.size(),
            Self::Eip4844(tx) => tx.size(),
            Self::Eip7702(tx) => tx.size(),
            #[cfg(feature = "optimism")]
            Self::Deposit(tx) => tx.size(),
        }
    }

    /// Returns true if the transaction is a legacy transaction.
    #[inline]
    pub const fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy(_))
    }

    /// Returns true if the transaction is an EIP-2930 transaction.
    #[inline]
    pub const fn is_eip2930(&self) -> bool {
        matches!(self, Self::Eip2930(_))
    }

    /// Returns true if the transaction is an EIP-1559 transaction.
    #[inline]
    pub const fn is_eip1559(&self) -> bool {
        matches!(self, Self::Eip1559(_))
    }

    /// Returns true if the transaction is an EIP-4844 transaction.
    #[inline]
    pub const fn is_eip4844(&self) -> bool {
        matches!(self, Self::Eip4844(_))
    }

    /// Returns true if the transaction is an EIP-7702 transaction.
    #[inline]
    pub const fn is_eip7702(&self) -> bool {
        matches!(self, Self::Eip7702(_))
    }

    /// Returns the [`TxLegacy`] variant if the transaction is a legacy transaction.
    pub const fn as_legacy(&self) -> Option<&TxLegacy> {
        match self {
            Self::Legacy(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip2930`] variant if the transaction is an EIP-2930 transaction.
    pub const fn as_eip2930(&self) -> Option<&TxEip2930> {
        match self {
            Self::Eip2930(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip1559`] variant if the transaction is an EIP-1559 transaction.
    pub const fn as_eip1559(&self) -> Option<&TxEip1559> {
        match self {
            Self::Eip1559(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip4844`] variant if the transaction is an EIP-4844 transaction.
    pub const fn as_eip4844(&self) -> Option<&TxEip4844> {
        match self {
            Self::Eip4844(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip7702`] variant if the transaction is an EIP-7702 transaction.
    pub const fn as_eip7702(&self) -> Option<&TxEip7702> {
        match self {
            Self::Eip7702(tx) => Some(tx),
            _ => None,
        }
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for Transaction {
    // Serializes the TxType to the buffer if necessary, returning 2 bits of the type as an
    // identifier instead of the length.
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let identifier = self.tx_type().to_compact(buf);
        match self {
            Self::Legacy(tx) => {
                tx.to_compact(buf);
            }
            Self::Eip2930(tx) => {
                tx.to_compact(buf);
            }
            Self::Eip1559(tx) => {
                tx.to_compact(buf);
            }
            Self::Eip4844(tx) => {
                tx.to_compact(buf);
            }
            Self::Eip7702(tx) => {
                tx.to_compact(buf);
            }
            #[cfg(feature = "optimism")]
            Self::Deposit(tx) => {
                tx.to_compact(buf);
            }
        }
        identifier
    }

    // For backwards compatibility purposes, only 2 bits of the type are encoded in the identifier
    // parameter. In the case of a [`COMPACT_EXTENDED_IDENTIFIER_FLAG`], the full transaction type
    // is read from the buffer as a single byte.
    //
    // # Panics
    //
    // A panic will be triggered if an identifier larger than 3 is passed from the database. For
    // optimism a identifier with value [`DEPOSIT_TX_TYPE_ID`] is allowed.
    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        match identifier {
            COMPACT_IDENTIFIER_LEGACY => {
                let (tx, buf) = TxLegacy::from_compact(buf, buf.len());
                (Self::Legacy(tx), buf)
            }
            COMPACT_IDENTIFIER_EIP2930 => {
                let (tx, buf) = TxEip2930::from_compact(buf, buf.len());
                (Self::Eip2930(tx), buf)
            }
            COMPACT_IDENTIFIER_EIP1559 => {
                let (tx, buf) = TxEip1559::from_compact(buf, buf.len());
                (Self::Eip1559(tx), buf)
            }
            COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                // An identifier of 3 indicates that the transaction type did not fit into
                // the backwards compatible 2 bit identifier, their transaction types are
                // larger than 2 bits (eg. 4844 and Deposit Transactions). In this case,
                // we need to read the concrete transaction type from the buffer by
                // reading the full 8 bits (single byte) and match on this transaction type.
                let identifier = buf.get_u8();
                match identifier {
                    EIP4844_TX_TYPE_ID => {
                        let (tx, buf) = TxEip4844::from_compact(buf, buf.len());
                        (Self::Eip4844(tx), buf)
                    }
                    EIP7702_TX_TYPE_ID => {
                        let (tx, buf) = TxEip7702::from_compact(buf, buf.len());
                        (Self::Eip7702(tx), buf)
                    }
                    #[cfg(feature = "optimism")]
                    DEPOSIT_TX_TYPE_ID => {
                        let (tx, buf) = TxDeposit::from_compact(buf, buf.len());
                        (Self::Deposit(tx), buf)
                    }
                    _ => unreachable!("Junk data in database: unknown Transaction variant"),
                }
            }
            _ => unreachable!("Junk data in database: unknown Transaction variant"),
        }
    }
}

impl Default for Transaction {
    fn default() -> Self {
        Self::Legacy(TxLegacy::default())
    }
}

impl Encodable for Transaction {
    /// This encodes the transaction _without_ the signature, and is only suitable for creating a
    /// hash intended for signing.
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        match self {
            Self::Legacy(legacy_tx) => {
                legacy_tx.encode_for_signing(out);
            }
            Self::Eip2930(access_list_tx) => {
                access_list_tx.encode_for_signing(out);
            }
            Self::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encode_for_signing(out);
            }
            Self::Eip4844(blob_tx) => {
                blob_tx.encode_for_signing(out);
            }
            Self::Eip7702(set_code_tx) => {
                set_code_tx.encode_for_signing(out);
            }
            #[cfg(feature = "optimism")]
            Self::Deposit(deposit_tx) => {
                deposit_tx.encode_inner(out, true);
            }
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::Legacy(legacy_tx) => legacy_tx.payload_len_for_signature(),
            Self::Eip2930(access_list_tx) => access_list_tx.payload_len_for_signature(),
            Self::Eip1559(dynamic_fee_tx) => dynamic_fee_tx.payload_len_for_signature(),
            Self::Eip4844(blob_tx) => blob_tx.payload_len_for_signature(),
            Self::Eip7702(set_code_tx) => set_code_tx.payload_len_for_signature(),
            #[cfg(feature = "optimism")]
            Self::Deposit(deposit_tx) => deposit_tx.encoded_len(true),
        }
    }
}

/// Signed transaction without its Hash. Used type for inserting into the DB.
///
/// This can by converted to [`TransactionSigned`] by calling [`TransactionSignedNoHash::hash`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize)]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
pub struct TransactionSignedNoHash {
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: Transaction,
}

impl TransactionSignedNoHash {
    /// Calculates the transaction hash. If used more than once, it's better to convert it to
    /// [`TransactionSigned`] first.
    pub fn hash(&self) -> B256 {
        // pre-allocate buffer for the transaction
        let mut buf = Vec::with_capacity(128 + self.transaction.input().len());
        self.transaction.encode_with_signature(&self.signature, &mut buf, false);
        keccak256(&buf)
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also [`Self::recover_signer`].
    pub fn recover_signer(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        #[cfg(feature = "optimism")]
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }

        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Reuses a given buffer to avoid numerous reallocations when recovering batches. **Clears the
    /// buffer before use.**
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// [`recover_signer_unchecked`].
    ///
    /// # Optimism
    ///
    /// For optimism this will return [`Address::ZERO`] if the Signature is empty, this is because pre bedrock (on OP mainnet), relay messages to the L2 Cross Domain Messenger were sent as legacy transactions from the zero address with an empty signature, e.g.: <https://optimistic.etherscan.io/tx/0x1bb352ff9215efe5a4c102f45d730bae323c3288d2636672eb61543ddd47abad>
    /// This makes it possible to import pre bedrock transactions via the sender recovery stage.
    pub fn encode_and_recover_unchecked(&self, buffer: &mut Vec<u8>) -> Option<Address> {
        buffer.clear();
        self.transaction.encode_without_signature(buffer);

        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        #[cfg(feature = "optimism")]
        {
            if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
                return Some(from)
            }

            // pre bedrock system transactions were sent from the zero address as legacy
            // transactions with an empty signature
            //
            // NOTE: this is very hacky and only relevant for op-mainnet pre bedrock
            if self.is_legacy() && self.signature == optimism_deposit_tx_signature() {
                return Some(Address::ZERO)
            }
        }

        recover_signer_unchecked(&self.signature, keccak256(buffer))
    }

    /// Converts into a transaction type with its hash: [`TransactionSigned`].
    ///
    /// Note: This will recalculate the hash of the transaction.
    #[inline]
    pub fn with_hash(self) -> TransactionSigned {
        let Self { signature, transaction } = self;
        TransactionSigned::from_transaction_and_signature(transaction, signature)
    }

    /// Recovers a list of signers from a transaction list iterator
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    pub fn recover_signers<'a, T>(txes: T, num_txes: usize) -> Option<Vec<Address>>
    where
        T: IntoParallelIterator<Item = &'a Self> + IntoIterator<Item = &'a Self> + Send,
    {
        if num_txes < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            txes.into_iter().map(|tx| tx.recover_signer()).collect()
        } else {
            txes.into_par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }
}

impl Default for TransactionSignedNoHash {
    fn default() -> Self {
        Self { signature: Signature::test_signature(), transaction: Default::default() }
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for TransactionSignedNoHash {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let tx_signed = TransactionSigned::arbitrary(u)?;

        Ok(Self { signature: tx_signed.signature, transaction: tx_signed.transaction })
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for TransactionSignedNoHash {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let start = buf.as_mut().len();

        // Placeholder for bitflags.
        // The first byte uses 4 bits as flags: IsCompressed[1bit], TxType[2bits], Signature[1bit]
        buf.put_u8(0);

        let sig_bit = self.signature.to_compact(buf) as u8;
        let zstd_bit = self.transaction.input().len() >= 32;

        let tx_bits = if zstd_bit {
            let mut tmp = Vec::with_capacity(256);
            if cfg!(feature = "std") {
                crate::compression::TRANSACTION_COMPRESSOR.with(|compressor| {
                    let mut compressor = compressor.borrow_mut();
                    let tx_bits = self.transaction.to_compact(&mut tmp);
                    buf.put_slice(&compressor.compress(&tmp).expect("Failed to compress"));
                    tx_bits as u8
                })
            } else {
                let mut compressor = crate::compression::create_tx_compressor();
                let tx_bits = self.transaction.to_compact(&mut tmp);
                buf.put_slice(&compressor.compress(&tmp).expect("Failed to compress"));
                tx_bits as u8
            }
        } else {
            self.transaction.to_compact(buf) as u8
        };

        // Replace bitflags with the actual values
        buf.as_mut()[start] = sig_bit | (tx_bits << 1) | ((zstd_bit as u8) << 3);

        buf.as_mut().len() - start
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        use bytes::Buf;

        // The first byte uses 4 bits as flags: IsCompressed[1], TxType[2], Signature[1]
        let bitflags = buf.get_u8() as usize;

        let sig_bit = bitflags & 1;
        let (mut signature, buf) = Signature::from_compact(buf, sig_bit);

        let zstd_bit = bitflags >> 3;
        let (transaction, buf) = if zstd_bit != 0 {
            if cfg!(feature = "std") {
                crate::compression::TRANSACTION_DECOMPRESSOR.with(|decompressor| {
                    let mut decompressor = decompressor.borrow_mut();

                    // TODO: enforce that zstd is only present at a "top" level type

                    let transaction_type = (bitflags & 0b110) >> 1;
                    let (transaction, _) =
                        Transaction::from_compact(decompressor.decompress(buf), transaction_type);

                    (transaction, buf)
                })
            } else {
                let mut decompressor = crate::compression::create_tx_decompressor();
                let transaction_type = (bitflags & 0b110) >> 1;
                let (transaction, _) =
                    Transaction::from_compact(decompressor.decompress(buf), transaction_type);

                (transaction, buf)
            }
        } else {
            let transaction_type = bitflags >> 1;
            Transaction::from_compact(buf, transaction_type)
        };

        if matches!(transaction, Transaction::Legacy(_)) {
            signature = signature.with_parity(legacy_parity(&signature, transaction.chain_id()))
        }

        (Self { signature, transaction }, buf)
    }
}

impl From<TransactionSignedNoHash> for TransactionSigned {
    fn from(tx: TransactionSignedNoHash) -> Self {
        tx.with_hash()
    }
}

impl From<TransactionSigned> for TransactionSignedNoHash {
    fn from(tx: TransactionSigned) -> Self {
        Self { signature: tx.signature, transaction: tx.transaction }
    }
}

/// Signed transaction.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize)]
pub struct TransactionSigned {
    /// Transaction hash
    pub hash: TxHash,
    /// The transaction signature values
    pub signature: Signature,
    /// Raw transaction info
    #[deref]
    #[as_ref]
    pub transaction: Transaction,
}

impl Default for TransactionSigned {
    fn default() -> Self {
        Self {
            hash: Default::default(),
            signature: Signature::test_signature(),
            transaction: Default::default(),
        }
    }
}

impl AsRef<Self> for TransactionSigned {
    fn as_ref(&self) -> &Self {
        self
    }
}

// === impl TransactionSigned ===

impl TransactionSigned {
    /// Transaction signature.
    pub const fn signature(&self) -> &Signature {
        &self.signature
    }

    /// Transaction hash. Used to identify transaction.
    pub const fn hash(&self) -> TxHash {
        self.hash
    }

    /// Reference to transaction hash. Used to identify transaction.
    pub const fn hash_ref(&self) -> &TxHash {
        &self.hash
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid following [EIP-2](https://eips.ethereum.org/EIPS/eip-2), see also [`recover_signer`].
    ///
    /// Note:
    ///
    /// This can fail for some early ethereum mainnet transactions pre EIP-2, use
    /// [`Self::recover_signer_unchecked`] if you want to recover the signer without ensuring that
    /// the signature has a low `s` value.
    pub fn recover_signer(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        #[cfg(feature = "optimism")]
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        recover_signer(&self.signature, signature_hash)
    }

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// [`recover_signer_unchecked`].
    pub fn recover_signer_unchecked(&self) -> Option<Address> {
        // Optimism's Deposit transaction does not have a signature. Directly return the
        // `from` address.
        #[cfg(feature = "optimism")]
        if let Transaction::Deposit(TxDeposit { from, .. }) = self.transaction {
            return Some(from)
        }
        let signature_hash = self.signature_hash();
        recover_signer_unchecked(&self.signature, signature_hash)
    }

    /// Recovers a list of signers from a transaction list iterator.
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    pub fn recover_signers<'a, T>(txes: T, num_txes: usize) -> Option<Vec<Address>>
    where
        T: IntoParallelIterator<Item = &'a Self> + IntoIterator<Item = &'a Self> + Send,
    {
        if num_txes < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            txes.into_iter().map(|tx| tx.recover_signer()).collect()
        } else {
            txes.into_par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }

    /// Recovers a list of signers from a transaction list iterator _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`Self::recover_signer_unchecked`].
    pub fn recover_signers_unchecked<'a, T>(txes: T, num_txes: usize) -> Option<Vec<Address>>
    where
        T: IntoParallelIterator<Item = &'a Self> + IntoIterator<Item = &'a Self>,
    {
        if num_txes < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            txes.into_iter().map(|tx| tx.recover_signer_unchecked()).collect()
        } else {
            txes.into_par_iter().map(|tx| tx.recover_signer_unchecked()).collect()
        }
    }

    /// Returns the [`TransactionSignedEcRecovered`] transaction with the given sender.
    #[inline]
    pub const fn with_signer(self, signer: Address) -> TransactionSignedEcRecovered {
        TransactionSignedEcRecovered::from_signed_transaction(self, signer)
    }

    /// Consumes the type, recover signer and return [`TransactionSignedEcRecovered`]
    ///
    /// Returns `None` if the transaction's signature is invalid, see also [`Self::recover_signer`].
    pub fn into_ecrecovered(self) -> Option<TransactionSignedEcRecovered> {
        let signer = self.recover_signer()?;
        Some(TransactionSignedEcRecovered { signed_transaction: self, signer })
    }

    /// Consumes the type, recover signer and return [`TransactionSignedEcRecovered`] _without
    /// ensuring that the signature has a low `s` value_ (EIP-2).
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer_unchecked`].
    pub fn into_ecrecovered_unchecked(self) -> Option<TransactionSignedEcRecovered> {
        let signer = self.recover_signer_unchecked()?;
        Some(TransactionSignedEcRecovered { signed_transaction: self, signer })
    }

    /// Tries to recover signer and return [`TransactionSignedEcRecovered`] by cloning the type.
    pub fn try_ecrecovered(&self) -> Option<TransactionSignedEcRecovered> {
        let signer = self.recover_signer()?;
        Some(TransactionSignedEcRecovered { signed_transaction: self.clone(), signer })
    }

    /// Tries to recover signer and return [`TransactionSignedEcRecovered`].
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    pub fn try_into_ecrecovered(self) -> Result<TransactionSignedEcRecovered, Self> {
        match self.recover_signer() {
            None => Err(self),
            Some(signer) => Ok(TransactionSignedEcRecovered { signed_transaction: self, signer }),
        }
    }

    /// Tries to recover signer and return [`TransactionSignedEcRecovered`]. _without ensuring that
    /// the signature has a low `s` value_ (EIP-2).
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [`Self::recover_signer_unchecked`].
    pub fn try_into_ecrecovered_unchecked(self) -> Result<TransactionSignedEcRecovered, Self> {
        match self.recover_signer_unchecked() {
            None => Err(self),
            Some(signer) => Ok(TransactionSignedEcRecovered { signed_transaction: self, signer }),
        }
    }

    /// Calculate transaction hash, eip2728 transaction does not contain rlp header and start with
    /// tx type.
    pub fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }

    /// Create a new signed transaction from a transaction and its signature.
    ///
    /// This will also calculate the transaction hash using its encoding.
    pub fn from_transaction_and_signature(transaction: Transaction, signature: Signature) -> Self {
        let mut initial_tx = Self { transaction, hash: Default::default(), signature };
        initial_tx.hash = initial_tx.recalculate_hash();
        initial_tx
    }

    /// Calculate a heuristic for the in-memory size of the [`TransactionSigned`].
    #[inline]
    pub fn size(&self) -> usize {
        mem::size_of::<TxHash>() + self.transaction.size() + mem::size_of::<Signature>()
    }

    /// Decodes legacy transaction from the data buffer into a tuple.
    ///
    /// This expects `rlp(legacy_tx)`
    ///
    /// Refer to the docs for [`Self::decode_rlp_legacy_transaction`] for details on the exact
    /// format expected.
    pub(crate) fn decode_rlp_legacy_transaction_tuple(
        data: &mut &[u8],
    ) -> alloy_rlp::Result<(TxLegacy, TxHash, Signature)> {
        // keep this around, so we can use it to calculate the hash
        let original_encoding = *data;

        let header = Header::decode(data)?;
        let remaining_len = data.len();

        let transaction_payload_len = header.payload_length;

        if transaction_payload_len > remaining_len {
            return Err(RlpError::InputTooShort)
        }

        let mut transaction = TxLegacy {
            nonce: Decodable::decode(data)?,
            gas_price: Decodable::decode(data)?,
            gas_limit: Decodable::decode(data)?,
            to: Decodable::decode(data)?,
            value: Decodable::decode(data)?,
            input: Decodable::decode(data)?,
            chain_id: None,
        };
        let (signature, extracted_id) = decode_with_eip155_chain_id(data)?;
        transaction.chain_id = extracted_id;

        // check the new length, compared to the original length and the header length
        let decoded = remaining_len - data.len();
        if decoded != transaction_payload_len {
            return Err(RlpError::UnexpectedLength)
        }

        let tx_length = header.payload_length + header.length();
        let hash = keccak256(&original_encoding[..tx_length]);
        Ok((transaction, hash, signature))
    }

    /// Decodes legacy transaction from the data buffer.
    ///
    /// This should be used _only_ be used in general transaction decoding methods, which have
    /// already ensured that the input is a legacy transaction with the following format:
    /// `rlp(legacy_tx)`
    ///
    /// Legacy transactions are encoded as lists, so the input should start with a RLP list header.
    ///
    /// This expects `rlp(legacy_tx)`
    // TODO: make buf advancement semantics consistent with `decode_enveloped_typed_transaction`,
    // so decoding methods do not need to manually advance the buffer
    pub fn decode_rlp_legacy_transaction(data: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let (transaction, hash, signature) = Self::decode_rlp_legacy_transaction_tuple(data)?;
        let signed = Self { transaction: Transaction::Legacy(transaction), hash, signature };
        Ok(signed)
    }
}

impl From<TransactionSignedEcRecovered> for TransactionSigned {
    fn from(recovered: TransactionSignedEcRecovered) -> Self {
        recovered.signed_transaction
    }
}

impl Encodable for TransactionSigned {
    /// This encodes the transaction _with_ the signature, and an rlp header.
    ///
    /// For legacy transactions, it encodes the transaction data:
    /// `rlp(tx-data)`
    ///
    /// For EIP-2718 typed transactions, it encodes the transaction type followed by the rlp of the
    /// transaction:
    /// `rlp(tx-type || rlp(tx-data))`
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        let mut payload_length = self.encode_2718_len();
        if !self.is_legacy() {
            payload_length += Header { list: false, payload_length }.length();
        }

        payload_length
    }
}

impl Decodable for TransactionSigned {
    /// This `Decodable` implementation only supports decoding rlp encoded transactions as it's used
    /// by p2p.
    ///
    /// The p2p encoding format always includes an RLP header, although the type RLP header depends
    /// on whether or not the transaction is a legacy transaction.
    ///
    /// If the transaction is a legacy transaction, it is just encoded as a RLP list:
    /// `rlp(tx-data)`.
    ///
    /// If the transaction is a typed transaction, it is encoded as a RLP string:
    /// `rlp(tx-type || rlp(tx-data))`
    ///
    /// This can be used for decoding all signed transactions in p2p `BlockBodies` responses.
    ///
    /// This cannot be used for decoding EIP-4844 transactions in p2p `PooledTransactions`, since
    /// the EIP-4844 variant of [`TransactionSigned`] does not include the blob sidecar.
    ///
    /// For a method suitable for decoding pooled transactions, see [`PooledTransactionsElement`].
    ///
    /// CAUTION: Due to a quirk in [`Header::decode`], this method will succeed even if a typed
    /// transaction is encoded in this format, and does not start with a RLP header:
    /// `tx-type || rlp(tx-data)`.
    ///
    /// This is because [`Header::decode`] does not advance the buffer, and returns a length-1
    /// string header if the first byte is less than `0xf7`.
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf).map_err(Into::into)
    }
}

impl Encodable2718 for TransactionSigned {
    fn type_flag(&self) -> Option<u8> {
        match self.transaction.tx_type() {
            TxType::Legacy => None,
            tx_type => Some(tx_type as u8),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match &self.transaction {
            Transaction::Legacy(legacy_tx) => legacy_tx.encoded_len_with_signature(
                &with_eip155_parity(&self.signature, legacy_tx.chain_id),
            ),
            Transaction::Eip2930(access_list_tx) => {
                access_list_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Eip1559(dynamic_fee_tx) => {
                dynamic_fee_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Eip4844(blob_tx) => {
                blob_tx.encoded_len_with_signature(&self.signature, false)
            }
            Transaction::Eip7702(set_code_tx) => {
                set_code_tx.encoded_len_with_signature(&self.signature, false)
            }
            #[cfg(feature = "optimism")]
            Transaction::Deposit(deposit_tx) => deposit_tx.encoded_len(false),
        }
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.transaction.encode_with_signature(&self.signature, out, false)
    }
}

impl Decodable2718 for TransactionSigned {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            TxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
            TxType::Eip2930 => {
                let (tx, signature, hash) = TxEip2930::decode_signed_fields(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip2930(tx), signature, hash })
            }
            TxType::Eip1559 => {
                let (tx, signature, hash) = TxEip1559::decode_signed_fields(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip1559(tx), signature, hash })
            }
            TxType::Eip7702 => {
                let (tx, signature, hash) = TxEip7702::decode_signed_fields(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip7702(tx), signature, hash })
            }
            TxType::Eip4844 => {
                let (tx, signature, hash) = TxEip4844::decode_signed_fields(buf)?.into_parts();
                Ok(Self { transaction: Transaction::Eip4844(tx), signature, hash })
            }
            #[cfg(feature = "optimism")]
            TxType::Deposit => Ok(Self::from_transaction_and_signature(
                Transaction::Deposit(TxDeposit::decode(buf)?),
                optimism_deposit_tx_signature(),
            )),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::decode_rlp_legacy_transaction(buf)?)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for TransactionSigned {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[allow(unused_mut)]
        let mut transaction = Transaction::arbitrary(u)?;

        let secp = secp256k1::Secp256k1::new();
        let key_pair = secp256k1::Keypair::new(&secp, &mut rand::thread_rng());
        let mut signature = crate::sign_message(
            B256::from_slice(&key_pair.secret_bytes()[..]),
            transaction.signature_hash(),
        )
        .unwrap();

        signature = if matches!(transaction, Transaction::Legacy(_)) {
            if let Some(chain_id) = transaction.chain_id() {
                signature.with_chain_id(chain_id)
            } else {
                signature.with_parity(alloy_primitives::Parity::NonEip155(bool::arbitrary(u)?))
            }
        } else {
            signature.with_parity_bool()
        };

        #[cfg(feature = "optimism")]
        // Both `Some(0)` and `None` values are encoded as empty string byte. This introduces
        // ambiguity in roundtrip tests. Patch the mint value of deposit transaction here, so that
        // it's `None` if zero.
        if let Transaction::Deposit(ref mut tx_deposit) = transaction {
            if tx_deposit.mint == Some(0) {
                tx_deposit.mint = None;
            }
        }

        #[cfg(feature = "optimism")]
        let signature =
            if transaction.is_deposit() { optimism_deposit_tx_signature() } else { signature };

        Ok(Self::from_transaction_and_signature(transaction, signature))
    }
}

/// Signed transaction with recovered signer.
#[derive(Debug, Clone, PartialEq, Hash, Eq, AsRef, Deref)]
pub struct TransactionSignedEcRecovered {
    /// Signer of the transaction
    signer: Address,
    /// Signed transaction
    #[deref]
    #[as_ref]
    signed_transaction: TransactionSigned,
}

// === impl TransactionSignedEcRecovered ===

impl TransactionSignedEcRecovered {
    /// Signer of transaction recovered from signature
    pub const fn signer(&self) -> Address {
        self.signer
    }

    /// Transform back to [`TransactionSigned`]
    pub fn into_signed(self) -> TransactionSigned {
        self.signed_transaction
    }

    /// Dissolve Self to its component
    pub fn to_components(self) -> (TransactionSigned, Address) {
        (self.signed_transaction, self.signer)
    }

    /// Create [`TransactionSignedEcRecovered`] from [`TransactionSigned`] and [`Address`] of the
    /// signer.
    #[inline]
    pub const fn from_signed_transaction(
        signed_transaction: TransactionSigned,
        signer: Address,
    ) -> Self {
        Self { signed_transaction, signer }
    }
}

impl Encodable for TransactionSignedEcRecovered {
    /// This encodes the transaction _with_ the signature, and an rlp header.
    ///
    /// Refer to docs for [`TransactionSigned::encode`] for details on the exact format.
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        self.signed_transaction.encode(out)
    }

    fn length(&self) -> usize {
        self.signed_transaction.length()
    }
}

impl Decodable for TransactionSignedEcRecovered {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let signed_transaction = TransactionSigned::decode(buf)?;
        let signer = signed_transaction
            .recover_signer()
            .ok_or(RlpError::Custom("Unable to recover decoded transaction signer."))?;
        Ok(Self { signer, signed_transaction })
    }
}

/// Generic wrapper with encoded Bytes, such as transaction data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithEncoded<T>(Bytes, pub T);

impl<T> From<(Bytes, T)> for WithEncoded<T> {
    fn from(value: (Bytes, T)) -> Self {
        Self(value.0, value.1)
    }
}

impl<T> WithEncoded<T> {
    /// Wraps the value with the bytes.
    pub const fn new(bytes: Bytes, value: T) -> Self {
        Self(bytes, value)
    }

    /// Get the encoded bytes
    pub fn encoded_bytes(&self) -> Bytes {
        self.0.clone()
    }

    /// Get the underlying value
    pub const fn value(&self) -> &T {
        &self.1
    }

    /// Returns ownership of the underlying value.
    pub fn into_value(self) -> T {
        self.1
    }

    /// Transform the value
    pub fn transform<F: From<T>>(self) -> WithEncoded<F> {
        WithEncoded(self.0, self.1.into())
    }

    /// Split the wrapper into [`Bytes`] and value tuple
    pub fn split(self) -> (Bytes, T) {
        (self.0, self.1)
    }

    /// Maps the inner value to a new value using the given function.
    pub fn map<U, F: FnOnce(T) -> U>(self, op: F) -> WithEncoded<U> {
        WithEncoded(self.0, op(self.1))
    }
}

impl<T> WithEncoded<Option<T>> {
    /// returns `None` if the inner value is `None`, otherwise returns `Some(WithEncoded<T>)`.
    pub fn transpose(self) -> Option<WithEncoded<T>> {
        self.1.map(|v| WithEncoded(self.0, v))
    }
}

/// Bincode-compatible transaction type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    use alloc::borrow::Cow;
    use alloy_consensus::{
        transaction::serde_bincode_compat::{TxEip1559, TxEip2930, TxEip7702, TxLegacy},
        TxEip4844,
    };
    use alloy_primitives::{Signature, TxHash};
    #[cfg(feature = "optimism")]
    use op_alloy_consensus::serde_bincode_compat::TxDeposit;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::Transaction`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives::{serde_bincode_compat, Transaction};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::transaction::Transaction")]
    ///     transaction: Transaction,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    #[allow(missing_docs)]
    pub enum Transaction<'a> {
        Legacy(TxLegacy<'a>),
        Eip2930(TxEip2930<'a>),
        Eip1559(TxEip1559<'a>),
        Eip4844(Cow<'a, TxEip4844>),
        Eip7702(TxEip7702<'a>),
        #[cfg(feature = "optimism")]
        #[cfg(feature = "optimism")]
        Deposit(TxDeposit<'a>),
    }

    impl<'a> From<&'a super::Transaction> for Transaction<'a> {
        fn from(value: &'a super::Transaction) -> Self {
            match value {
                super::Transaction::Legacy(tx) => Self::Legacy(TxLegacy::from(tx)),
                super::Transaction::Eip2930(tx) => Self::Eip2930(TxEip2930::from(tx)),
                super::Transaction::Eip1559(tx) => Self::Eip1559(TxEip1559::from(tx)),
                super::Transaction::Eip4844(tx) => Self::Eip4844(Cow::Borrowed(tx)),
                super::Transaction::Eip7702(tx) => Self::Eip7702(TxEip7702::from(tx)),
                #[cfg(feature = "optimism")]
                super::Transaction::Deposit(tx) => Self::Deposit(TxDeposit::from(tx)),
            }
        }
    }

    impl<'a> From<Transaction<'a>> for super::Transaction {
        fn from(value: Transaction<'a>) -> Self {
            match value {
                Transaction::Legacy(tx) => Self::Legacy(tx.into()),
                Transaction::Eip2930(tx) => Self::Eip2930(tx.into()),
                Transaction::Eip1559(tx) => Self::Eip1559(tx.into()),
                Transaction::Eip4844(tx) => Self::Eip4844(tx.into_owned()),
                Transaction::Eip7702(tx) => Self::Eip7702(tx.into()),
                #[cfg(feature = "optimism")]
                Transaction::Deposit(tx) => Self::Deposit(tx.into()),
            }
        }
    }

    impl SerializeAs<super::Transaction> for Transaction<'_> {
        fn serialize_as<S>(source: &super::Transaction, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Transaction::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::Transaction> for Transaction<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::Transaction, D::Error>
        where
            D: Deserializer<'de>,
        {
            Transaction::deserialize(deserializer).map(Into::into)
        }
    }

    /// Bincode-compatible [`super::TransactionSigned`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives::{serde_bincode_compat, TransactionSigned};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::transaction::TransactionSigned")]
    ///     transaction: TransactionSigned,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct TransactionSigned<'a> {
        hash: TxHash,
        signature: Signature,
        transaction: Transaction<'a>,
    }

    impl<'a> From<&'a super::TransactionSigned> for TransactionSigned<'a> {
        fn from(value: &'a super::TransactionSigned) -> Self {
            Self {
                hash: value.hash,
                signature: value.signature,
                transaction: Transaction::from(&value.transaction),
            }
        }
    }

    impl<'a> From<TransactionSigned<'a>> for super::TransactionSigned {
        fn from(value: TransactionSigned<'a>) -> Self {
            Self {
                hash: value.hash,
                signature: value.signature,
                transaction: value.transaction.into(),
            }
        }
    }

    impl SerializeAs<super::TransactionSigned> for TransactionSigned<'_> {
        fn serialize_as<S>(
            source: &super::TransactionSigned,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            TransactionSigned::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::TransactionSigned> for TransactionSigned<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::TransactionSigned, D::Error>
        where
            D: Deserializer<'de>,
        {
            TransactionSigned::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, Transaction, TransactionSigned};

        use arbitrary::Arbitrary;
        use rand::Rng;
        use reth_testing_utils::generators;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_transaction_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::Transaction")]
                transaction: Transaction,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let data = Data {
                transaction: Transaction::arbitrary(&mut arbitrary::Unstructured::new(&bytes))
                    .unwrap(),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }

        #[test]
        fn test_transaction_signed_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::TransactionSigned")]
                transaction: TransactionSigned,
            }

            let mut bytes = [0u8; 1024];
            generators::rng().fill(bytes.as_mut_slice());
            let data = Data {
                transaction: TransactionSigned::arbitrary(&mut arbitrary::Unstructured::new(
                    &bytes,
                ))
                .unwrap(),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        transaction::{signature::Signature, TxEip1559, TxKind, TxLegacy},
        Transaction, TransactionSigned, TransactionSignedEcRecovered, TransactionSignedNoHash,
    };
    use alloy_eips::eip2718::{Decodable2718, Encodable2718};
    use alloy_primitives::{address, b256, bytes, hex, Address, Bytes, Parity, B256, U256};
    use alloy_rlp::{Decodable, Encodable, Error as RlpError};
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_codecs::Compact;
    use std::str::FromStr;

    #[test]
    fn test_decode_empty_typed_tx() {
        let input = [0x80u8];
        let res = TransactionSigned::decode(&mut &input[..]).unwrap_err();
        assert_eq!(RlpError::InputTooShort, res);
    }

    #[test]
    fn raw_kind_encoding_sanity() {
        // check the 0x80 encoding for Create
        let mut buf = Vec::new();
        TxKind::Create.encode(&mut buf);
        assert_eq!(buf, vec![0x80]);

        // check decoding
        let buf = [0x80];
        let decoded = TxKind::decode(&mut &buf[..]).unwrap();
        assert_eq!(decoded, TxKind::Create);
    }

    #[test]
    fn test_decode_create_goerli() {
        // test that an example create tx from goerli decodes properly
        let tx_bytes = hex!("b901f202f901ee05228459682f008459682f11830209bf8080b90195608060405234801561001057600080fd5b50610175806100206000396000f3fe608060405234801561001057600080fd5b506004361061002b5760003560e01c80630c49c36c14610030575b600080fd5b61003861004e565b604051610045919061011d565b60405180910390f35b60606020600052600f6020527f68656c6c6f2073746174656d696e64000000000000000000000000000000000060405260406000f35b600081519050919050565b600082825260208201905092915050565b60005b838110156100be5780820151818401526020810190506100a3565b838111156100cd576000848401525b50505050565b6000601f19601f8301169050919050565b60006100ef82610084565b6100f9818561008f565b93506101098185602086016100a0565b610112816100d3565b840191505092915050565b6000602082019050818103600083015261013781846100e4565b90509291505056fea264697066735822122051449585839a4ea5ac23cae4552ef8a96b64ff59d0668f76bfac3796b2bdbb3664736f6c63430008090033c080a0136ebffaa8fc8b9fda9124de9ccb0b1f64e90fbd44251b4c4ac2501e60b104f9a07eb2999eec6d185ef57e91ed099afb0a926c5b536f0155dd67e537c7476e1471");

        let decoded = TransactionSigned::decode(&mut &tx_bytes[..]).unwrap();
        assert_eq!(tx_bytes.len(), decoded.length());
        assert_eq!(tx_bytes, &alloy_rlp::encode(decoded)[..]);
    }

    #[test]
    fn test_decode_recover_mainnet_tx() {
        // random mainnet tx <https://etherscan.io/tx/0x86718885c4b4218c6af87d3d0b0d83e3cc465df2a05c048aa4db9f1a6f9de91f>
        let tx_bytes = hex!("02f872018307910d808507204d2cb1827d0094388c818ca8b9251b393131c08a736a67ccb19297880320d04823e2701c80c001a0cf024f4815304df2867a1a74e9d2707b6abda0337d2d54a4438d453f4160f190a07ac0e6b3bc9395b5b9c8b9e6d77204a236577a5b18467b9175c01de4faa208d9");

        let decoded = TransactionSigned::decode_2718(&mut &tx_bytes[..]).unwrap();
        assert_eq!(
            decoded.recover_signer(),
            Some(Address::from_str("0x95222290DD7278Aa3Ddd389Cc1E1d165CC4BAfe5").unwrap())
        );
    }

    #[test]
    // Test vector from https://sepolia.etherscan.io/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    // Blobscan: https://sepolia.blobscan.com/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    fn test_decode_recover_sepolia_4844_tx() {
        use crate::TxType;
        use alloy_primitives::{address, b256};

        // https://sepolia.etherscan.io/getRawTx?tx=0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
        let raw_tx = alloy_primitives::hex::decode("0x03f9011d83aa36a7820fa28477359400852e90edd0008252089411e9ca82a3a762b4b5bd264d4173a242e7a770648080c08504a817c800f8a5a0012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921aa00152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4a0013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7a001148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1a0011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e654901a0c8de4cced43169f9aa3d36506363b2d2c44f6c49fc1fd91ea114c86f3757077ea01e11fdd0d1934eda0492606ee0bb80a7bf8f35cc5f86ec60fe5031ba48bfd544").unwrap();
        let decoded = TransactionSigned::decode_2718(&mut raw_tx.as_slice()).unwrap();
        assert_eq!(decoded.tx_type(), TxType::Eip4844);

        let from = decoded.recover_signer();
        assert_eq!(from, Some(address!("A83C816D4f9b2783761a22BA6FADB0eB0606D7B2")));

        let tx = decoded.transaction;

        assert_eq!(tx.to(), Some(address!("11E9CA82A3a762b4B5bd264d4173a242e7a77064")));

        assert_eq!(
            tx.blob_versioned_hashes(),
            Some(vec![
                b256!("012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921a"),
                b256!("0152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4"),
                b256!("013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7"),
                b256!("01148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1"),
                b256!("011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e6549"),
            ])
        );
    }

    #[test]
    fn decode_transaction_consumes_buffer() {
        let bytes = &mut &hex!("b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469")[..];
        let _transaction_res = TransactionSigned::decode(bytes).unwrap();
        assert_eq!(
            bytes.len(),
            0,
            "did not consume all bytes in the buffer, {:?} remaining",
            bytes.len()
        );
    }

    #[test]
    fn decode_multiple_network_txs() {
        let bytes = hex!("f86b02843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba00eb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5aea03a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4u64),
            nonce: 2,
            gas_price: 1000000000,
            gas_limit: 100000,
            to: Address::from_str("d3e8763675e4c425df46cc3b5c0f6cbdac396046").unwrap().into(),
            value: U256::from(1000000000000000u64),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae")
                .unwrap(),
            U256::from_str("0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18")
                .unwrap(),
            Parity::Eip155(43),
        );
        let hash = b256!("a517b206d2223278f860ea017d3626cacad4f52ff51030dc9a96b432f17f8d34");
        test_decode_and_encode(&bytes, transaction, signature, Some(hash));

        let bytes = hex!("f86b01843b9aca00830186a094d3e8763675e4c425df46cc3b5c0f6cbdac3960468702769bb01b2a00802ba0e24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0aa05406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 1u64,
            gas_price: 1000000000,
            gas_limit: 100000,
            to: Address::from_slice(&hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046")[..]).into(),
            value: U256::from(693361000000000u64),
            input: Default::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xe24d8bd32ad906d6f8b8d7741e08d1959df021698b19ee232feba15361587d0a")
                .unwrap(),
            U256::from_str("0x5406ad177223213df262cb66ccbb2f46bfdccfdfbbb5ffdda9e2c02d977631da")
                .unwrap(),
            Parity::Eip155(43),
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("f86b0384773594008398968094d3e8763675e4c425df46cc3b5c0f6cbdac39604687038d7ea4c68000802ba0ce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071a03ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 3,
            gas_price: 2000000000,
            gas_limit: 10000000,
            to: Address::from_slice(&hex!("d3e8763675e4c425df46cc3b5c0f6cbdac396046")[..]).into(),
            value: U256::from(1000000000000000u64),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0xce6834447c0a4193c40382e6c57ae33b241379c5418caac9cdc18d786fd12071")
                .unwrap(),
            U256::from_str("0x3ca3ae86580e94550d7c071e3a02eadb5a77830947c9225165cf9100901bee88")
                .unwrap(),
            Parity::Eip155(43),
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("b87502f872041a8459682f008459682f0d8252089461815774383099e24810ab832a5b2a5425c154d58829a2241af62c000080c001a059e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafda0016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469");
        let transaction = Transaction::Eip1559(TxEip1559 {
            chain_id: 4,
            nonce: 26,
            max_priority_fee_per_gas: 1500000000,
            max_fee_per_gas: 1500000013,
            gas_limit: MIN_TRANSACTION_GAS,
            to: Address::from_slice(&hex!("61815774383099e24810ab832a5b2a5425c154d5")[..]).into(),
            value: U256::from(3000000000000000000u64),
            input: Default::default(),
            access_list: Default::default(),
        });
        let signature = Signature::new(
            U256::from_str("0x59e6b67f48fb32e7e570dfb11e042b5ad2e55e3ce3ce9cd989c7e06e07feeafd")
                .unwrap(),
            U256::from_str("0x016b83f4f980694ed2eee4d10667242b1f40dc406901b34125b008d334d47469")
                .unwrap(),
            Parity::Parity(true),
        );
        test_decode_and_encode(&bytes, transaction, signature, None);

        let bytes = hex!("f8650f84832156008287fb94cf7f9e66af820a19257a2108375b180b0ec491678204d2802ca035b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981a0612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860");
        let transaction = Transaction::Legacy(TxLegacy {
            chain_id: Some(4),
            nonce: 15,
            gas_price: 2200000000,
            gas_limit: 34811,
            to: Address::from_slice(&hex!("cf7f9e66af820a19257a2108375b180b0ec49167")[..]).into(),
            value: U256::from(1234),
            input: Bytes::default(),
        });
        let signature = Signature::new(
            U256::from_str("0x35b7bfeb9ad9ece2cbafaaf8e202e706b4cfaeb233f46198f00b44d4a566a981")
                .unwrap(),
            U256::from_str("0x612638fb29427ca33b9a3be2a0a561beecfe0269655be160d35e72d366a6a860")
                .unwrap(),
            Parity::Eip155(44),
        );
        test_decode_and_encode(&bytes, transaction, signature, None);
    }

    fn test_decode_and_encode(
        bytes: &[u8],
        transaction: Transaction,
        signature: Signature,
        hash: Option<B256>,
    ) {
        let expected = TransactionSigned::from_transaction_and_signature(transaction, signature);
        if let Some(hash) = hash {
            assert_eq!(hash, expected.hash);
        }
        assert_eq!(bytes.len(), expected.length());

        let decoded = TransactionSigned::decode(&mut &bytes[..]).unwrap();
        assert_eq!(expected, decoded);
        assert_eq!(bytes, &alloy_rlp::encode(expected));
    }

    #[test]
    fn decode_raw_tx_and_recover_signer() {
        use alloy_primitives::hex_literal::hex;
        // transaction is from ropsten

        let hash: B256 =
            hex!("559fb34c4a7f115db26cbf8505389475caaab3df45f5c7a0faa4abfa3835306c").into();
        let signer: Address = hex!("641c5d790f862a58ec7abcfd644c0442e9c201b3").into();
        let raw = hex!("f88b8212b085028fa6ae00830f424094aad593da0c8116ef7d2d594dd6a63241bccfc26c80a48318b64b000000000000000000000000641c5d790f862a58ec7abcfd644c0442e9c201b32aa0a6ef9e170bca5ffb7ac05433b13b7043de667fbb0b4a5e45d3b54fb2d6efcc63a0037ec2c05c3d60c5f5f78244ce0a3859e3a18a36c61efb061b383507d3ce19d2");

        let mut pointer = raw.as_ref();
        let tx = TransactionSigned::decode(&mut pointer).unwrap();
        assert_eq!(tx.hash(), hash, "Expected same hash");
        assert_eq!(tx.recover_signer(), Some(signer), "Recovering signer should pass.");
    }

    #[test]
    fn test_envelop_encode() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let decoded = TransactionSigned::decode(&mut &input[..]).unwrap();

        let encoded = decoded.encoded_2718();
        assert_eq!(encoded[..], input);
    }

    #[test]
    fn test_envelop_decode() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = bytes!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let decoded = TransactionSigned::decode_2718(&mut input.as_ref()).unwrap();

        let encoded = decoded.encoded_2718();
        assert_eq!(encoded, input);
    }

    #[test]
    fn test_decode_signed_ec_recovered_transaction() {
        // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
        let input = hex!("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76");
        let tx = TransactionSigned::decode(&mut &input[..]).unwrap();
        let recovered = tx.into_ecrecovered().unwrap();

        let decoded =
            TransactionSignedEcRecovered::decode(&mut &alloy_rlp::encode(&recovered)[..]).unwrap();
        assert_eq!(recovered, decoded)
    }

    #[test]
    fn test_decode_tx() {
        // some random transactions pulled from hive tests
        let data = hex!("b86f02f86c0705843b9aca008506fc23ac00830124f89400000000000000000000000000000000000003160180c001a00293c713e2f1eab91c366621ff2f867e05ad7e99d4aa5d069aafeb9e1e8c9b6aa05ec6c0605ff20b57c90a6484ec3b0509e5923733d06f9b69bee9a2dabe4f1352");
        let tx = TransactionSigned::decode(&mut data.as_slice()).unwrap();
        let mut b = Vec::with_capacity(data.len());
        tx.encode(&mut b);
        assert_eq!(data.as_slice(), b.as_slice());

        let data = hex!("f865048506fc23ac00830124f8940000000000000000000000000000000000000316018032a06b8fdfdcb84790816b7af85b19305f493665fe8b4e7c51ffdd7cc144cd776a60a028a09ab55def7b8d6602ba1c97a0ebbafe64ffc9c8e89520cec97a8edfb2ebe9");
        let tx = TransactionSigned::decode(&mut data.as_slice()).unwrap();
        let mut b = Vec::with_capacity(data.len());
        tx.encode(&mut b);
        assert_eq!(data.as_slice(), b.as_slice());
    }

    #[cfg(feature = "secp256k1")]
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(1))]

        #[test]
        fn test_parallel_recovery_order(txes in proptest::collection::vec(
            proptest_arbitrary_interop::arb::<Transaction>(),
            *crate::transaction::PARALLEL_SENDER_RECOVERY_THRESHOLD * 5
        )) {
            let mut rng =rand::thread_rng();
            let secp = secp256k1::Secp256k1::new();
            let txes: Vec<TransactionSigned> = txes.into_iter().map(|mut tx| {
                 if let Some(chain_id) = tx.chain_id() {
                    // Otherwise we might overflow when calculating `v` on `recalculate_hash`
                    tx.set_chain_id(chain_id % (u64::MAX / 2 - 36));
                }

                let key_pair = secp256k1::Keypair::new(&secp, &mut rng);

                let signature =
                    crate::sign_message(B256::from_slice(&key_pair.secret_bytes()[..]), tx.signature_hash()).unwrap();

                TransactionSigned::from_transaction_and_signature(tx, signature)
            }).collect();

            let parallel_senders = TransactionSigned::recover_signers(&txes, txes.len()).unwrap();
            let seq_senders = txes.iter().map(|tx| tx.recover_signer()).collect::<Option<Vec<_>>>().unwrap();

            assert_eq!(parallel_senders, seq_senders);
        }
    }

    // <https://etherscan.io/tx/0x280cde7cdefe4b188750e76c888f13bd05ce9a4d7767730feefe8a0e50ca6fc4>
    #[test]
    fn recover_legacy_singer() {
        let data = hex!("f9015482078b8505d21dba0083022ef1947a250d5630b4cf539739df2c5dacb4c659f2488d880c46549a521b13d8b8e47ff36ab50000000000000000000000000000000000000000000066ab5a608bd00a23f2fe000000000000000000000000000000000000000000000000000000000000008000000000000000000000000048c04ed5691981c42154c6167398f95e8f38a7ff00000000000000000000000000000000000000000000000000000000632ceac70000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000006c6ee5e31d828de241282b9606c8e98ea48526e225a0c9077369501641a92ef7399ff81c21639ed4fd8fc69cb793cfa1dbfab342e10aa0615facb2f1bcf3274a354cfe384a38d0cc008a11c2dd23a69111bc6930ba27a8");
        let tx = TransactionSigned::decode_rlp_legacy_transaction(&mut data.as_slice()).unwrap();
        assert!(tx.is_legacy());
        let sender = tx.recover_signer().unwrap();
        assert_eq!(sender, address!("a12e1462d0ceD572f396F58B6E2D03894cD7C8a4"));
    }

    // <https://github.com/alloy-rs/alloy/issues/141>
    // <https://etherscan.io/tx/0xce4dc6d7a7549a98ee3b071b67e970879ff51b5b95d1c340bacd80fa1e1aab31>
    #[test]
    fn recover_enveloped() {
        let data = hex!("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8");
        let tx = TransactionSigned::decode_2718(&mut data.as_slice()).unwrap();
        let sender = tx.recover_signer().unwrap();
        assert_eq!(sender, address!("001e2b7dE757bA469a57bF6b23d982458a07eFcE"));
        assert_eq!(tx.to(), Some(address!("D9e1459A7A482635700cBc20BBAF52D495Ab9C96")));
        assert_eq!(tx.input().as_ref(), hex!("1b55ba3a"));
        let encoded = tx.encoded_2718();
        assert_eq!(encoded.as_ref(), data.to_vec());
    }

    // <https://github.com/paradigmxyz/reth/issues/7750>
    // <https://etherscan.io/tx/0x2084b8144eea4031c2fa7dfe343498c5e665ca85ed17825f2925f0b5b01c36ac>
    #[test]
    fn recover_pre_eip2() {
        let data = hex!("f8ea0c850ba43b7400832dc6c0942935aa0a2d2fbb791622c29eb1c117b65b7a908580b884590528a9000000000000000000000001878ace42092b7f1ae1f28d16c1272b1aa80ca4670000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000d02ab486cedc0000000000000000000000000000000000000000000000000000557fe293cabc08cf1ca05bfaf3fda0a56b49cc78b22125feb5ae6a99d2b4781f00507d8b02c173771c85a0b5da0dbe6c5bc53740d0071fc83eb17ba0f709e49e9ae7df60dee625ef51afc5");
        let tx = TransactionSigned::decode_2718(&mut data.as_slice()).unwrap();
        let sender = tx.recover_signer();
        assert!(sender.is_none());
        let sender = tx.recover_signer_unchecked().unwrap();

        assert_eq!(sender, address!("7e9e359edf0dbacf96a9952fa63092d919b0842b"));
    }

    #[test]
    fn transaction_signed_no_hash_zstd_codec() {
        // will use same signature everywhere.
        // We don't need signature to match tx, just decoded to the same signature
        let signature = Signature::new(
            U256::from_str("0xeb96ca19e8a77102767a41fc85a36afd5c61ccb09911cec5d3e86e193d9c5ae")
                .unwrap(),
            U256::from_str("0x3a456401896b1b6055311536bf00a718568c744d8c1f9df59879e8350220ca18")
                .unwrap(),
            Parity::Eip155(43),
        );

        let inputs: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![255],
            vec![1u8; 31],
            vec![255u8; 31],
            vec![1u8; 32],
            vec![255u8; 32],
            vec![1u8; 64],
            vec![255u8; 64],
        ];

        for input in inputs {
            let transaction = Transaction::Legacy(TxLegacy {
                chain_id: Some(4u64),
                nonce: 2,
                gas_price: 1000000000,
                gas_limit: 100000,
                to: Address::from_str("d3e8763675e4c425df46cc3b5c0f6cbdac396046").unwrap().into(),
                value: U256::from(1000000000000000u64),
                input: Bytes::from(input),
            });

            let tx_signed_no_hash = TransactionSignedNoHash { signature, transaction };
            test_transaction_signed_to_from_compact(tx_signed_no_hash);
        }
    }

    fn test_transaction_signed_to_from_compact(tx_signed_no_hash: TransactionSignedNoHash) {
        // zstd aware `to_compact`
        let mut buff: Vec<u8> = Vec::new();
        let written_bytes = tx_signed_no_hash.to_compact(&mut buff);
        let (decoded, _) = TransactionSignedNoHash::from_compact(&buff, written_bytes);
        assert_eq!(tx_signed_no_hash, decoded);
    }

    #[test]
    fn create_txs_disallowed_for_eip4844() {
        let data =
            [3, 208, 128, 128, 123, 128, 120, 128, 129, 129, 128, 192, 129, 129, 192, 128, 128, 9];
        let res = TransactionSigned::decode_2718(&mut &data[..]);

        assert!(res.is_err());
    }
}
