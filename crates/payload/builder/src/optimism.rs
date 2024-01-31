use crate::EthPayloadBuilderAttributes;
use alloy_rlp::{Encodable, Error as DecodeError};
use reth_node_api::PayloadBuilderAttributes;
use reth_primitives::{Address, TransactionSigned, Withdrawal, B256};
use reth_rpc_types::engine::{OptimismPayloadAttributes, PayloadId};
use reth_rpc_types_compat::engine::payload::convert_standalone_withdraw_to_withdrawal;

/// Optimism Payload Builder Attributes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimismPayloadBuilderAttributes {
    /// Inner ethereum payload builder attributes
    pub payload_attributes: EthPayloadBuilderAttributes,
    /// NoTxPool option for the generated payload
    pub no_tx_pool: bool,
    /// Transactions for the generated payload
    pub transactions: Vec<TransactionSigned>,
    /// The gas limit for the generated payload
    pub gas_limit: Option<u64>,
}

impl PayloadBuilderAttributes for OptimismPayloadBuilderAttributes {
    type RpcPayloadAttributes = OptimismPayloadAttributes;
    type Error = DecodeError;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    fn try_new(parent: B256, attributes: OptimismPayloadAttributes) -> Result<Self, DecodeError> {
        let (id, transactions) = {
            let transactions: Vec<_> = attributes
                .transactions
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|tx| TransactionSigned::decode_enveloped(&mut tx.as_ref()))
                .collect::<Result<_, _>>()?;
            (payload_id_optimism(&parent, &attributes, &transactions), transactions)
        };

        let withdraw = attributes.payload_attributes.withdrawals.map(
            |withdrawals: Vec<reth_rpc_types::withdrawal::Withdrawal>| {
                withdrawals
                    .into_iter()
                    .map(convert_standalone_withdraw_to_withdrawal) // Removed the parentheses here
                    .collect::<Vec<_>>()
            },
        );

        let payload_attributes = EthPayloadBuilderAttributes {
            id,
            parent,
            timestamp: attributes.payload_attributes.timestamp,
            suggested_fee_recipient: attributes.payload_attributes.suggested_fee_recipient,
            prev_randao: attributes.payload_attributes.prev_randao,
            withdrawals: withdraw.unwrap_or_default(),
            parent_beacon_block_root: attributes.payload_attributes.parent_beacon_block_root,
        };

        Ok(Self {
            payload_attributes,
            no_tx_pool: attributes.no_tx_pool.unwrap_or_default(),
            transactions,
            gas_limit: attributes.gas_limit,
        })
    }

    fn payload_id(&self) -> PayloadId {
        self.payload_attributes.id
    }

    fn parent(&self) -> B256 {
        self.payload_attributes.parent
    }

    fn timestamp(&self) -> u64 {
        self.payload_attributes.timestamp
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.payload_attributes.parent_beacon_block_root
    }

    fn suggested_fee_recipient(&self) -> Address {
        self.payload_attributes.suggested_fee_recipient
    }

    fn prev_randao(&self) -> B256 {
        self.payload_attributes.prev_randao
    }

    fn withdrawals(&self) -> &Vec<Withdrawal> {
        &self.payload_attributes.withdrawals
    }
}

/// Generates the payload id for the configured payload from the [OptimismPayloadAttributes].
///
/// Returns an 8-byte identifier by hashing the payload components with sha256 hash.
pub(crate) fn payload_id_optimism(
    parent: &B256,
    attributes: &OptimismPayloadAttributes,
    txs: &[TransactionSigned],
) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_slice());
    hasher.update(&attributes.payload_attributes.timestamp.to_be_bytes()[..]);
    hasher.update(attributes.payload_attributes.prev_randao.as_slice());
    hasher.update(attributes.payload_attributes.suggested_fee_recipient.as_slice());
    if let Some(withdrawals) = &attributes.payload_attributes.withdrawals {
        let mut buf = Vec::new();
        withdrawals.encode(&mut buf);
        hasher.update(buf);
    }

    if let Some(parent_beacon_block) = attributes.payload_attributes.parent_beacon_block_root {
        hasher.update(parent_beacon_block);
    }

    let no_tx_pool = attributes.no_tx_pool.unwrap_or_default();
    if no_tx_pool || !txs.is_empty() {
        hasher.update([no_tx_pool as u8]);
        hasher.update(txs.len().to_be_bytes());
        txs.iter().for_each(|tx| hasher.update(tx.hash()));
    }

    if let Some(gas_limit) = attributes.gas_limit {
        hasher.update(gas_limit.to_be_bytes());
    }

    let out = hasher.finalize();
    PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
}
