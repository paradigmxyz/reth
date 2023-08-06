
use reth_primitives::{
    AccessListItem, Address, BlockNumber, Bytes, Transaction as PrimitiveTransaction,
    TransactionKind as PrimitiveTransactionKind, TransactionSignedEcRecovered, TxType, H256, U128,
    U256, U64,
};

use reth_rpc_types::Signature;

use serde::{Deserialize, Serialize};

/// Transaction object used in RPC
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    /// Hash
    pub hash: H256,
    /// Nonce
    pub nonce: U256,
    /// Block hash
    pub block_hash: Option<H256>,
    /// Block number
    pub block_number: Option<U256>,
    /// Transaction Index
    pub transaction_index: Option<U256>,
    /// Sender
    pub from: Address,
    /// Recipient
    pub to: Option<Address>,
    /// Transferred value
    pub value: U256,
    /// Gas Price
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_price: Option<U128>,
    /// Gas amount
    pub gas: U256,
    /// Max BaseFeePerGas the user is willing to pay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_fee_per_gas: Option<U128>,
    /// The miner's tip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_priority_fee_per_gas: Option<U128>,
    /// Data
    pub input: Bytes,
    /// All _flattened_ fields of the transaction signature.
    ///
    /// Note: this is an option so special transaction types without a signature (e.g. <https://github.com/ethereum-optimism/optimism/blob/0bf643c4147b43cd6f25a759d331ef3a2a61a2a3/specs/deposits.md#the-deposited-transaction-type>) can be supported.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    /// The chain id of the transaction, if any.
    pub chain_id: Option<U64>,
    /// EIP2930
    ///
    /// Pre-pay to warm storage access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_list: Option<Vec<AccessListItem>>,
    /// EIP2718
    ///
    /// Transaction type, Some(2) for EIP-1559 transaction,
    /// Some(1) for AccessList transaction, None for Legacy
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub transaction_type: Option<U64>,
}

impl Transaction {
    /// Create a new rpc transaction result for a mined transaction, using the given block hash,
    /// number, and tx index fields to populate the corresponding fields in the rpc result.
    ///
    /// The block hash, number, and tx index fields should be from the original block where the
    /// transaction was mined.
    pub fn from_recovered_with_block_context(
        tx: TransactionSignedEcRecovered,
        block_hash: H256,
        block_number: BlockNumber,
        base_fee: Option<u64>,
        tx_index: U256,
    ) -> Self {
        Self::fill(tx, Some(block_hash), Some(block_number), base_fee, Some(tx_index))
    }

    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    pub fn from_recovered(tx: TransactionSignedEcRecovered) -> Self {
        Self::fill(tx, None, None, None, None)
    }

    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn fill(
        tx: TransactionSignedEcRecovered,
        block_hash: Option<H256>,
        block_number: Option<BlockNumber>,
        base_fee: Option<u64>,
        transaction_index: Option<U256>,
    ) -> Self {
        let signer = tx.signer();
        let signed_tx = tx.into_signed();

        let to = match signed_tx.kind() {
            PrimitiveTransactionKind::Create => None,
            PrimitiveTransactionKind::Call(to) => Some(*to),
        };

        let (gas_price, max_fee_per_gas) = match signed_tx.tx_type() {
            TxType::Legacy => (Some(U128::from(signed_tx.max_fee_per_gas())), None),
            TxType::EIP2930 => (Some(U128::from(signed_tx.max_fee_per_gas())), None),
            TxType::EIP1559 | TxType::EIP4844 => {
                // the gas price field for EIP1559 is set to `min(tip, gasFeeCap - baseFee) +
                // baseFee`
                let gas_price = base_fee
                    .and_then(|base_fee| {
                        signed_tx.effective_tip_per_gas(base_fee).map(|tip| tip + base_fee as u128)
                    })
                    .unwrap_or_else(|| signed_tx.max_fee_per_gas());

                (Some(U128::from(gas_price)), Some(U128::from(signed_tx.max_fee_per_gas())))
            }
        };

        let chain_id = signed_tx.chain_id().map(U64::from);
        let access_list = match &signed_tx.transaction {
            PrimitiveTransaction::Legacy(_) => None,
            PrimitiveTransaction::Eip2930(tx) => Some(
                tx.access_list
                    .0
                    .iter()
                    .map(|item| AccessListItem {
                        address: item.address.0.into(),
                        storage_keys: item.storage_keys.iter().map(|key| key.0.into()).collect(),
                    })
                    .collect(),
            ),
            PrimitiveTransaction::Eip1559(tx) => Some(
                tx.access_list
                    .0
                    .iter()
                    .map(|item| AccessListItem {
                        address: item.address.0.into(),
                        storage_keys: item.storage_keys.iter().map(|key| key.0.into()).collect(),
                    })
                    .collect(),
            ),
        };

        let signature = Signature::from_primitive_signature(
            *signed_tx.signature(),
            signed_tx.tx_type(),
            signed_tx.chain_id(),
        );

        Self {
            hash: signed_tx.hash(),
            nonce: U256::from(signed_tx.nonce()),
            from: signer,
            to,
            value: U256::from(signed_tx.value()),
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas: signed_tx.max_priority_fee_per_gas().map(U128::from),
            signature: Some(signature),
            gas: U256::from(signed_tx.gas_limit()),
            input: signed_tx.input().clone(),
            chain_id,
            access_list,
            transaction_type: Some(U64::from(signed_tx.tx_type() as u8)),

            // These fields are set to None because they are not stored as part of the transaction
            block_hash,
            block_number: block_number.map(U256::from),
            transaction_index,
        }
    }
}




