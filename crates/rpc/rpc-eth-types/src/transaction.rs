//! Helper types for `reth_rpc_eth_api::EthApiServer` implementation.
//!
//! Transaction wrapper that labels transaction with its origin.

use crate::EthApiError;
use alloy_consensus::transaction::TxHashRef;
use alloy_eips::{
    eip8141::{FrameMode, LEANSPHINCS_VERIFIER, P256_VERIFIER, SECP256K1_VERIFIER},
    eip8250::{nonce_slot, NONCE_MANAGER},
};
use alloy_primitives::{Address, B256, U256};
use alloy_rpc_types_eth::{FrameRequest, TransactionInfo, TransactionRequest};
use evm2::{version::GasId, Version};
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::{NodePrimitives, Recovered, SignedTransaction};
use reth_rpc_convert::{RpcConvert, RpcTransaction};

/// Represents from where a transaction was fetched.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransactionSource<T = TransactionSigned> {
    /// Transaction exists in the pool (Pending)
    Pool(Recovered<T>),
    /// Transaction already included in a block
    ///
    /// This can be a historical block or a pending block (received from the CL)
    Block {
        /// Transaction fetched via provider
        transaction: Recovered<T>,
        /// Index of the transaction in the block
        index: u64,
        /// Hash of the block.
        block_hash: B256,
        /// Number of the block.
        block_number: u64,
        /// Timestamp of the block.
        block_timestamp: u64,
        /// base fee of the block.
        base_fee: Option<u64>,
    },
}

// === impl TransactionSource ===

impl<T: SignedTransaction> TransactionSource<T> {
    /// Consumes the type and returns the wrapped transaction.
    pub fn into_recovered(self) -> Recovered<T> {
        self.into()
    }

    /// Conversion into network specific transaction type.
    pub fn into_transaction<Builder>(
        self,
        resp_builder: &Builder,
    ) -> Result<RpcTransaction<Builder::Network>, Builder::Error>
    where
        Builder: RpcConvert<Primitives: NodePrimitives<SignedTx = T>>,
    {
        match self {
            Self::Pool(tx) => resp_builder.fill_pending(tx),
            Self::Block {
                transaction,
                index,
                block_hash,
                block_number,
                block_timestamp,
                base_fee,
            } => {
                let tx_info = TransactionInfo {
                    hash: Some(*transaction.tx_hash()),
                    index: Some(index),
                    block_hash: Some(block_hash),
                    block_number: Some(block_number),
                    block_timestamp: Some(block_timestamp),
                    base_fee,
                };

                resp_builder.fill(transaction, tx_info)
            }
        }
    }

    /// Returns the transaction and block related info, if not pending
    pub fn split(self) -> (Recovered<T>, TransactionInfo) {
        match self {
            Self::Pool(tx) => {
                let hash = *tx.tx_hash();
                (tx, TransactionInfo { hash: Some(hash), ..Default::default() })
            }
            Self::Block {
                transaction,
                index,
                block_hash,
                block_number,
                block_timestamp,
                base_fee,
            } => {
                let hash = *transaction.tx_hash();
                (
                    transaction,
                    TransactionInfo {
                        hash: Some(hash),
                        index: Some(index),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        block_timestamp: Some(block_timestamp),
                        base_fee,
                    },
                )
            }
        }
    }
}

impl<T> From<TransactionSource<T>> for Recovered<T> {
    fn from(value: TransactionSource<T>) -> Self {
        match value {
            TransactionSource::Pool(tx) => tx,
            TransactionSource::Block { transaction, .. } => transaction,
        }
    }
}

/// Returns whether proof data prevents mutating fields covered by the signing hash.
/// Only omitted proofs for known protocol verifiers are simulation placeholders.
pub fn has_frame_proofs(request: &TransactionRequest) -> bool {
    request.frames.as_deref().is_some_and(|frames| {
        frames.iter().any(|frame| {
            frame.mode == FrameMode::Proof &&
                (frame.data.len() > 64 ||
                    !matches!(
                        frame.target.address(),
                        Some(SECP256K1_VERIFIER | P256_VERIFIER | LEANSPHINCS_VERIFIER)
                    ))
        })
    })
}

/// Reserves calldata gas for omitted fixed-length protocol proofs using the active schedule.
/// Returns zero for contract proofs or complete witnesses, and errors on gas overflow.
pub fn frame_proof_padding_gas(
    frames: &[FrameRequest],
    version: &Version,
) -> Result<u64, EthApiError> {
    let overflow = || EthApiError::InvalidParams("proof data gas overflows".into());
    let bytes = frames
        .iter()
        .filter(|frame| frame.mode == FrameMode::Proof && frame.data.len() == 64)
        .filter_map(|frame| match frame.target.address()? {
            SECP256K1_VERIFIER => Some(65u64),
            P256_VERIFIER => Some(128u64),
            LEANSPHINCS_VERIFIER => Some(7740u64),
            _ => None,
        })
        .try_fold(0u64, |sum, length| sum.checked_add(length))
        .ok_or_else(overflow)?;
    if bytes == 0 {
        return Ok(0);
    }
    let gp = &version.gas_params;
    let tokens = gp
        .get(GasId::TxTokenNonZeroByteMultiplier)
        .max(gp.get(GasId::TxFloorZeroByteMultiplier))
        .max(1);
    bytes
        .checked_mul(tokens)
        .and_then(|tokens| {
            tokens.checked_mul(gp.get(GasId::TxTokenCost).max(gp.get(GasId::TxFloorCostPerToken)))
        })
        .ok_or_else(overflow)
}

/// Reads the shared sequence for nonce keys already validated by Alloy.
/// Empty keys return `None`; unequal sequences return `NonceKeysNotSynchronized`.
/// Callers enforce their own exhaustion policy in `storage`, preserving read-error precedence.
pub fn read_frame_nonce(
    address: Address,
    keys: &[U256],
    mut storage: impl FnMut(Address, U256) -> Result<U256, EthApiError>,
) -> Result<Option<U256>, EthApiError> {
    let mut sequence = None;
    for key in keys {
        let current = storage(NONCE_MANAGER, nonce_slot(address, *key))?;
        if sequence.is_some_and(|previous| previous != current) {
            return Err(EthApiError::NonceKeysNotSynchronized);
        }
        sequence = Some(current);
    }
    Ok(sequence)
}

/// Fills an omitted frame nonce from its keys without changing explicit nonces or legacy requests.
/// Rejects exhausted or unequal keyed sequences before converting them to `u64`.
pub fn resolve_frame_nonce(
    request: &mut TransactionRequest,
    mut storage: impl FnMut(Address, U256) -> Result<U256, EthApiError>,
) -> Result<(), EthApiError> {
    if !request.minimal_tx_type().is_eip8141() || request.nonce.is_some() {
        return Ok(());
    }
    let Some(keys) = request.nonce_keys.as_ref() else { return Ok(()) };
    alloy_eips::eip8250::validate_nonce_keys(keys)
        .map_err(|reason| EthApiError::InvalidParams(reason.into()))?;
    let invalid =
        || EthApiError::InvalidParams("nonce keys require one non-exhausted sequence".into());
    let sequence = read_frame_nonce(request.from.unwrap_or_default(), keys, |address, slot| {
        let sequence = storage(address, slot)?;
        if sequence >= U256::from(u64::MAX) {
            return Err(invalid());
        }
        Ok(sequence)
    })
    .map_err(|error| match error {
        EthApiError::NonceKeysNotSynchronized => invalid(),
        error => error,
    })?;
    if let Some(sequence) = sequence {
        request.nonce = Some(sequence.to::<u64>());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip8141::{Frame, FrameMode};
    use alloy_primitives::Bytes;
    use evm2::{ethereum::eip8141::FrameGas, SpecId};

    #[test]
    fn frame_proof_guards_distinguish_complete_witnesses_and_native_placeholders() {
        for (target, length, expected) in [
            (SECP256K1_VERIFIER, 64, false),
            (P256_VERIFIER, 64, false),
            (LEANSPHINCS_VERIFIER, 64, false),
            (SECP256K1_VERIFIER, 129, true),
            (Address::repeat_byte(7), 64, true),
        ] {
            let request = TransactionRequest {
                frames: Some(vec![FrameRequest {
                    mode: FrameMode::Proof,
                    target: target.into(),
                    data: Bytes::from(vec![1; length]),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            assert_eq!(has_frame_proofs(&request), expected);
        }
    }

    #[test]
    fn frame_proof_padding_covers_native_gas_for_completed_witnesses() {
        for custom in [false, true] {
            let mut version = Version::new(SpecId::BOGOTA);
            if custom {
                version.gas_params.set(GasId::TxTokenNonZeroByteMultiplier, 1);
                version.gas_params.set(GasId::TxFloorZeroByteMultiplier, 4);
            }
            let mut frames: Vec<FrameRequest> =
                [SECP256K1_VERIFIER, P256_VERIFIER, LEANSPHINCS_VERIFIER]
                    .into_iter()
                    .map(|target| FrameRequest {
                        mode: FrameMode::Proof,
                        target: target.into(),
                        execution_gas: Some(20000),
                        state_gas: Some(0),
                        data: Bytes::from(vec![1; 64]),
                        ..Default::default()
                    })
                    .collect();
            let mut tx = alloy_consensus::TxEip8141 {
                frames: frames
                    .iter()
                    .cloned()
                    .map(Frame::try_from)
                    .collect::<Result<_, _>>()
                    .unwrap(),
                ..Default::default()
            };
            let unsigned = FrameGas::calculate(&tx, &version).unwrap();
            let padding = frame_proof_padding_gas(&frames, &version).unwrap();
            for (frame, length) in frames.iter_mut().zip([65, 128, 7740]) {
                frame.data = Bytes::from(vec![1; 64 + length]);
            }
            tx.frames =
                frames.iter().cloned().map(Frame::try_from).collect::<Result<_, _>>().unwrap();
            let completed = FrameGas::calculate(&tx, &version).unwrap();
            assert!(completed.maximum > unsigned.maximum);
            assert!(unsigned.maximum + padding >= completed.maximum);
            assert_eq!(frame_proof_padding_gas(&frames, &version).unwrap(), 0);
        }
    }

    #[test]
    fn frame_proof_padding_rejects_overflow_only_when_witnesses_are_missing() {
        let mut version = Version::new(SpecId::BOGOTA);
        version.gas_params.set(GasId::TxTokenNonZeroByteMultiplier, u32::MAX);
        version.gas_params.set(GasId::TxTokenCost, u32::MAX);
        let frames = [FrameRequest {
            mode: FrameMode::Proof,
            target: SECP256K1_VERIFIER.into(),
            data: Bytes::from(vec![1; 64]),
            ..Default::default()
        }];
        assert_eq!(
            frame_proof_padding_gas(&frames, &version).unwrap_err().to_string(),
            "proof data gas overflows"
        );
        assert_eq!(frame_proof_padding_gas(&[], &version).unwrap(), 0);
    }

    #[test]
    fn keyed_nonce_reads_preserve_slots_errors_and_exhaustion_policy() {
        let sender = Address::repeat_byte(0x41);
        let keys = [U256::ZERO, U256::ONE];
        let mut slots = Vec::new();
        assert_eq!(
            read_frame_nonce(sender, &keys, |address, slot| {
                assert_eq!(address, NONCE_MANAGER);
                slots.push(slot);
                Ok(U256::from(u64::MAX))
            })
            .unwrap(),
            Some(U256::from(u64::MAX))
        );
        assert_eq!(slots, keys.map(|key| nonce_slot(sender, key)).to_vec());
        assert_eq!(read_frame_nonce(sender, &[], |_, _| unreachable!()).unwrap(), None);
        assert!(matches!(
            read_frame_nonce(sender, &keys, |_, _| Err(EthApiError::InternalEthError)),
            Err(EthApiError::InternalEthError)
        ));
        let mut values = [U256::ONE, U256::from(2)].into_iter();
        assert!(matches!(
            read_frame_nonce(sender, &keys, |_, _| Ok(values.next().unwrap())),
            Err(EthApiError::NonceKeysNotSynchronized)
        ));

        let mut request = TransactionRequest {
            transaction_type: Some(6),
            from: Some(sender),
            nonce_keys: Some(keys.to_vec()),
            ..Default::default()
        };
        let mut reads = 0;
        let error = resolve_frame_nonce(&mut request, |_, _| {
            reads += 1;
            Ok(U256::from(u64::MAX))
        })
        .unwrap_err();
        assert_eq!(reads, 1);
        assert_eq!(error.to_string(), "nonce keys require one non-exhausted sequence");
    }
}
