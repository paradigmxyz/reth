//! Validates execution payload wrt Ethereum consensus rules

use alloy_consensus::Block;
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{ExecutionData, PayloadError};
use reth_chainspec::EthereumHardforks;
use reth_payload_validator::{cancun, prague, shanghai};
use reth_primitives_traits::{Block as _, SealedBlock, SignedTransaction};
use std::sync::{mpsc, Arc};
use tracing::{debug, debug_span};

/// Execution payload validator.
#[derive(Clone, Debug)]
pub struct EthereumExecutionPayloadValidator<ChainSpec> {
    /// Chain spec to validate against.
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> EthereumExecutionPayloadValidator<ChainSpec> {
    /// Create a new validator.
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }

    /// Returns the chain spec used by the validator.
    #[inline]
    pub const fn chain_spec(&self) -> &Arc<ChainSpec> {
        &self.chain_spec
    }
}

impl<ChainSpec: EthereumHardforks> EthereumExecutionPayloadValidator<ChainSpec> {
    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout,
    ///
    /// See also [`ensure_well_formed_payload`]
    pub fn ensure_well_formed_payload<T: SignedTransaction>(
        &self,
        payload: ExecutionData,
    ) -> Result<SealedBlock<Block<T>>, PayloadError> {
        ensure_well_formed_payload(&self.chain_spec, payload)
    }

    /// Streaming variant of [`Self::ensure_well_formed_payload`] that takes decoded transactions
    /// from `txs` instead of decoding them from the payload.
    ///
    /// See also [`ensure_well_formed_payload_with_tx_stream`]
    pub fn ensure_well_formed_payload_with_tx_stream<T: SignedTransaction>(
        &self,
        payload: ExecutionData,
        txs: mpsc::Receiver<(usize, T)>,
    ) -> Result<(SealedBlock<Block<T>>, B256), PayloadError> {
        ensure_well_formed_payload_with_tx_stream(&self.chain_spec, payload, txs)
    }
}

/// Ensures that the given payload does not violate any consensus rules that concern the block's
/// layout, like:
///    - missing or invalid base fee
///    - invalid extra data
///    - invalid transactions
///    - incorrect hash
///    - the versioned hashes passed with the payload do not exactly match transaction versioned
///      hashes
///    - the block does not contain blob transactions if it is pre-cancun
///
/// The checks are done in the order that conforms with the engine-API specification.
///
/// This is intended to be invoked after receiving the payload from the CLI.
/// The additional [`MaybeCancunPayloadFields`](alloy_rpc_types_engine::MaybeCancunPayloadFields) are not part of the payload, but are additional fields in the `engine_newPayloadV3` RPC call, See also <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#engine_newpayloadv3>
///
/// If the cancun fields are provided this also validates that the versioned hashes in the block
/// match the versioned hashes passed in the
/// [`CancunPayloadFields`](alloy_rpc_types_engine::CancunPayloadFields), if the cancun payload
/// fields are provided. If the payload fields are not provided, but versioned hashes exist
/// in the block, this is considered an error: [`PayloadError::InvalidVersionedHashes`].
///
/// This validates versioned hashes according to the Engine API Cancun spec:
/// <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification>
pub fn ensure_well_formed_payload<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: ExecutionData,
) -> Result<SealedBlock<Block<T>>, PayloadError>
where
    ChainSpec: EthereumHardforks,
    T: SignedTransaction,
{
    let ExecutionData { payload, sidecar } = payload;

    let expected_hash = payload.block_hash();

    // First parse the block
    let sealed_block = payload.try_into_block_with_sidecar(&sidecar)?.seal_slow();

    // Ensure the hash included in the payload matches the block hash
    if expected_hash != sealed_block.hash() {
        return Err(PayloadError::BlockHash {
            execution: sealed_block.hash(),
            consensus: expected_hash,
        })
    }

    shanghai::ensure_well_formed_fields(
        sealed_block.body(),
        chain_spec.is_shanghai_active_at_timestamp(sealed_block.timestamp),
    )?;

    cancun::ensure_well_formed_fields(
        &sealed_block,
        sidecar.cancun(),
        chain_spec.is_cancun_active_at_timestamp(sealed_block.timestamp),
    )?;

    prague::ensure_well_formed_fields(
        sealed_block.body(),
        sidecar.prague(),
        chain_spec.is_prague_active_at_timestamp(sealed_block.timestamp),
    )?;

    Ok(sealed_block)
}

/// Streaming variant of [`ensure_well_formed_payload`] that assembles the block body from
/// transactions decoded by the engine's execution-side fan-out instead of decoding them again.
///
/// The block hash is validated before any transaction handling, so a payload with both a bad hash
/// and a malformed transaction reports [`PayloadError::BlockHash`]. The Engine API lists the block
/// hash check first; the non-streaming path reports the decode error instead because it must
/// decode transactions to compute the hash input.
///
/// If `txs` disconnects before all transactions arrive (e.g. a malformed transaction stopped the
/// decoder, or execution was aborted), this falls back to decoding the retained raw transaction
/// bytes, reproducing the errors of [`ensure_well_formed_payload`].
///
/// Returns the sealed block together with the transactions root computed from the raw payload
/// bytes, so callers can skip re-encoding transactions for root validation.
pub fn ensure_well_formed_payload_with_tx_stream<ChainSpec, T>(
    chain_spec: ChainSpec,
    payload: ExecutionData,
    txs: mpsc::Receiver<(usize, T)>,
) -> Result<(SealedBlock<Block<T>>, B256), PayloadError>
where
    ChainSpec: EthereumHardforks,
    T: SignedTransaction,
{
    let ExecutionData { payload, sidecar } = payload;

    let expected_hash = payload.block_hash();

    // Build the block with raw transaction bytes; the transactions root in the header is
    // computed directly from the raw bytes, so transactions are never re-encoded.
    let raw_block = payload.into_block_with_sidecar_raw(&sidecar)?;
    let transactions_root = raw_block.header.transactions_root;

    // Ensure the hash included in the payload matches the block hash before waiting on any
    // transactions, so header validation overlaps with the execution-side decode.
    let hash = raw_block.header.hash_slow();
    if expected_hash != hash {
        return Err(PayloadError::BlockHash { execution: hash, consensus: expected_hash })
    }

    let Block { header, body: raw_body } = raw_block;

    let transactions = {
        let _span = debug_span!(
            target: "engine::tree::payload_validator",
            "assemble_body_from_stream",
        )
        .entered();
        let transaction_count = raw_body.transactions.len();
        let mut slots: Vec<Option<T>> = Vec::new();
        slots.resize_with(transaction_count, || None);
        let mut received = 0usize;

        loop {
            if received == transaction_count {
                break slots.into_iter().flatten().collect()
            }
            match txs.recv() {
                Ok((idx, tx)) => {
                    // The parallel fan-out sends out of order, hence index-addressed slots.
                    if let Some(slot) = slots.get_mut(idx) &&
                        slot.replace(tx).is_none()
                    {
                        received += 1;
                    }
                }
                Err(_) => {
                    // The decoder died before all transactions arrived (malformed transaction,
                    // aborted execution, or an engine early-return). Decode from the retained
                    // raw bytes to reproduce the non-streaming behavior.
                    debug!(
                        target: "engine::tree::payload_validator",
                        received,
                        transaction_count,
                        "payload tx stream disconnected, falling back to full decode"
                    );
                    break decode_transactions(&raw_body.transactions)?
                }
            }
        }
    };

    let block = Block {
        header,
        body: alloy_consensus::BlockBody {
            transactions,
            ommers: vec![],
            withdrawals: raw_body.withdrawals,
        },
    };
    // The header hash was already computed and verified above, no need to reseal.
    let sealed_block = SealedBlock::new_unchecked(block, hash);

    shanghai::ensure_well_formed_fields(
        sealed_block.body(),
        chain_spec.is_shanghai_active_at_timestamp(sealed_block.timestamp),
    )?;

    cancun::ensure_well_formed_fields(
        &sealed_block,
        sidecar.cancun(),
        chain_spec.is_cancun_active_at_timestamp(sealed_block.timestamp),
    )?;

    prague::ensure_well_formed_fields(
        sealed_block.body(),
        sidecar.prague(),
        chain_spec.is_prague_active_at_timestamp(sealed_block.timestamp),
    )?;

    Ok((sealed_block, transactions_root))
}

/// Decodes raw payload transactions, mirroring the errors produced by
/// [`ExecutionPayload::try_into_block_with_sidecar`](alloy_rpc_types_engine::ExecutionPayload::try_into_block_with_sidecar).
fn decode_transactions<T: SignedTransaction>(raw: &[Bytes]) -> Result<Vec<T>, PayloadError> {
    raw.iter()
        .map(|tx| {
            T::decode_2718_exact(tx.as_ref())
                .map_err(alloy_rlp::Error::from)
                .map_err(PayloadError::from)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{BlockBody, SignableTransaction, TxLegacy};
    use alloy_primitives::{Address, Signature, TxKind, B256, U256};
    use alloy_rpc_types_engine::{ExecutionPayloadSidecar, ExecutionPayloadV1};
    use reth_chainspec::MAINNET;
    use reth_ethereum_primitives::TransactionSigned;

    fn signed_tx(nonce: u64) -> TransactionSigned {
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: 7,
            gas_limit: 21_000,
            to: TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Default::default(),
        };
        tx.into_signed(Signature::test_signature()).into()
    }

    /// Builds a payload whose advertised block hash is consistent with its (possibly garbage)
    /// raw transaction bytes.
    fn payload_with_raw_txs(raw_txs: Vec<Bytes>) -> ExecutionData {
        let block: Block<TransactionSigned> =
            Block { header: Default::default(), body: BlockBody::default() };
        let mut payload = ExecutionPayloadV1::from_block_unchecked(B256::ZERO, &block);
        payload.transactions = raw_txs;
        payload.block_hash = payload.clone().into_block_raw().unwrap().header.hash_slow();
        ExecutionData { payload: payload.into(), sidecar: ExecutionPayloadSidecar::none() }
    }

    fn payload_with_txs(txs: &[TransactionSigned]) -> ExecutionData {
        use alloy_eips::eip2718::Encodable2718;
        payload_with_raw_txs(txs.iter().map(|tx| tx.encoded_2718().into()).collect())
    }

    #[test]
    fn assembles_body_from_out_of_order_stream() {
        let txs: Vec<_> = (0..3).map(signed_tx).collect();
        let payload = payload_with_txs(&txs);

        let expected: SealedBlock<Block<TransactionSigned>> =
            ensure_well_formed_payload(&*MAINNET, payload.clone()).unwrap();

        let (tx, rx) = mpsc::sync_channel(txs.len());
        for idx in [2, 0, 1] {
            tx.send((idx, txs[idx].clone())).unwrap();
        }
        drop(tx);

        let (block, tx_root) =
            ensure_well_formed_payload_with_tx_stream(&*MAINNET, payload, rx).unwrap();
        assert_eq!(block, expected);
        assert_eq!(tx_root, expected.transactions_root);
    }

    #[test]
    fn falls_back_to_decoding_on_disconnect() {
        let txs: Vec<_> = (0..3).map(signed_tx).collect();
        let payload = payload_with_txs(&txs);

        let expected: SealedBlock<Block<TransactionSigned>> =
            ensure_well_formed_payload(&*MAINNET, payload.clone()).unwrap();

        // Only one of three transactions arrives before the stream dies.
        let (tx, rx) = mpsc::sync_channel(txs.len());
        tx.send((0, txs[0].clone())).unwrap();
        drop(tx);

        let (block, _) = ensure_well_formed_payload_with_tx_stream(&*MAINNET, payload, rx).unwrap();
        assert_eq!(block, expected);
    }

    #[test]
    fn fallback_reproduces_decode_error() {
        // Garbage transaction bytes but a self-consistent block hash: the early hash check
        // passes and the fallback must report the same decode error as the non-streaming path.
        let payload = payload_with_raw_txs(vec![Bytes::from_static(b"garbage")]);

        let expected_err =
            ensure_well_formed_payload::<_, TransactionSigned>(&*MAINNET, payload.clone())
                .unwrap_err();

        let (tx, rx) = mpsc::sync_channel::<(usize, TransactionSigned)>(1);
        drop(tx);

        let err = ensure_well_formed_payload_with_tx_stream(&*MAINNET, payload, rx).unwrap_err();
        assert_eq!(format!("{err:?}"), format!("{expected_err:?}"));
        assert!(matches!(err, PayloadError::Decode(_)));
    }

    #[test]
    fn block_hash_checked_before_transactions() {
        // Both a bad hash and a malformed transaction: the streaming path reports the hash
        // mismatch because it validates the hash before touching transactions.
        let ExecutionData { payload, sidecar } =
            payload_with_raw_txs(vec![Bytes::from_static(b"garbage")]);
        let mut payload = payload.into_v1();
        payload.block_hash = B256::repeat_byte(0xff);
        let payload = ExecutionData { payload: payload.into(), sidecar };

        let (tx, rx) = mpsc::sync_channel::<(usize, TransactionSigned)>(1);
        drop(tx);

        let err = ensure_well_formed_payload_with_tx_stream(&*MAINNET, payload, rx).unwrap_err();
        assert!(matches!(err, PayloadError::BlockHash { .. }));
    }
}
