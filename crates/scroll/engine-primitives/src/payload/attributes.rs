//! Payload related types

use alloc::vec::Vec;
use std::fmt::Debug;

use alloy_eips::{eip2718::Decodable2718, eip4895::Withdrawals};
use alloy_primitives::{keccak256, Address, B256};
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::PayloadId;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_payload_primitives::PayloadBuilderAttributes;
use reth_primitives::transaction::WithEncoded;
use reth_scroll_primitives::ScrollTransactionSigned;
use scroll_alloy_rpc_types_engine::{BlockDataHint, ScrollPayloadAttributes};

/// Scroll Payload Builder Attributes
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScrollPayloadBuilderAttributes {
    /// Inner ethereum payload builder attributes
    pub payload_attributes: EthPayloadBuilderAttributes,
    /// `NoTxPool` option for the generated payload
    pub no_tx_pool: bool,
    /// Decoded transactions and the original EIP-2718 encoded bytes as received in the payload
    /// attributes.
    pub transactions: Vec<WithEncoded<ScrollTransactionSigned>>,
    /// The pre-Euclid block data hint, necessary for the block builder to derive the correct block
    /// hash.
    pub block_data_hint: Option<BlockDataHint>,
    /// The gas limit for the generated payload.
    pub gas_limit: Option<u64>,
}

impl PayloadBuilderAttributes for ScrollPayloadBuilderAttributes {
    type RpcPayloadAttributes = ScrollPayloadAttributes;
    type Error = alloy_rlp::Error;

    fn try_new(
        parent: B256,
        attributes: ScrollPayloadAttributes,
        version: u8,
    ) -> Result<Self, Self::Error> {
        let id = payload_id_scroll(&parent, &attributes, version);

        let transactions = attributes
            .transactions
            .unwrap_or_default()
            .into_iter()
            .map(|data| {
                let mut buf = data.as_ref();
                let tx = Decodable2718::decode_2718(&mut buf).map_err(alloy_rlp::Error::from)?;

                if !buf.is_empty() {
                    return Err(alloy_rlp::Error::UnexpectedLength);
                }

                Ok(WithEncoded::new(data, tx))
            })
            .collect::<Result<_, _>>()?;

        let payload_attributes = EthPayloadBuilderAttributes {
            id,
            parent,
            timestamp: attributes.payload_attributes.timestamp,
            suggested_fee_recipient: attributes.payload_attributes.suggested_fee_recipient,
            prev_randao: attributes.payload_attributes.prev_randao,
            withdrawals: attributes.payload_attributes.withdrawals.unwrap_or_default().into(),
            parent_beacon_block_root: attributes.payload_attributes.parent_beacon_block_root,
        };

        Ok(Self {
            payload_attributes,
            no_tx_pool: attributes.no_tx_pool,
            transactions,
            block_data_hint: attributes.block_data_hint,
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

    fn withdrawals(&self) -> &Withdrawals {
        &self.payload_attributes.withdrawals
    }
}

/// Generates the payload id for the configured payload from the [`ScrollPayloadAttributes`].
///
/// Returns an 8-byte identifier by hashing the payload components with sha256 hash.
pub(crate) fn payload_id_scroll(
    parent: &B256,
    attributes: &ScrollPayloadAttributes,
    payload_version: u8,
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

    let no_tx_pool = attributes.no_tx_pool;
    if no_tx_pool || attributes.transactions.as_ref().is_some_and(|txs| !txs.is_empty()) {
        hasher.update([no_tx_pool as u8]);
        let txs_len = attributes.transactions.as_ref().map(|txs| txs.len()).unwrap_or_default();
        hasher.update(&txs_len.to_be_bytes()[..]);
        if let Some(txs) = &attributes.transactions {
            for tx in txs {
                // we have to just hash the bytes here because otherwise we would need to decode
                // the transactions here which really isn't ideal
                let tx_hash = keccak256(tx);
                // maybe we can try just taking the hash and not decoding
                hasher.update(tx_hash)
            }
        }
    }

    if let Some(block_data) = &attributes.block_data_hint {
        hasher.update(&block_data.extra_data);
        hasher.update(block_data.state_root.0);
        if let Some(coinbase) = block_data.coinbase {
            hasher.update(coinbase);
        }
        if let Some(nonce) = block_data.nonce {
            hasher.update(nonce.to_be_bytes());
        }
        hasher.update(block_data.difficulty.to_be_bytes::<32>());
    }

    if let Some(gas_limit) = attributes.gas_limit {
        hasher.update(gas_limit.to_be_bytes());
    }

    let mut out = hasher.finalize();
    out[0] = payload_version;
    PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
}

impl From<EthPayloadBuilderAttributes> for ScrollPayloadBuilderAttributes {
    fn from(value: EthPayloadBuilderAttributes) -> Self {
        Self { payload_attributes: value, ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::str::FromStr;
    use alloy_primitives::{address, b256, bytes, FixedBytes, U256};
    use alloy_rpc_types_engine::PayloadAttributes;
    use reth_payload_primitives::EngineApiMessageVersion;

    #[test]
    fn test_payload_id() {
        let expected =
            PayloadId::new(FixedBytes::<8>::from_str("0x036369370c155d4c").unwrap().into());
        let attrs = ScrollPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 1728933301,
                prev_randao: b256!("9158595abbdab2c90635087619aa7042bbebe47642dfab3c9bfb934f6b082765"),
                suggested_fee_recipient: address!("4200000000000000000000000000000000000011"),
                withdrawals: Some([].into()),
                parent_beacon_block_root: b256!("8fe0193b9bf83cb7e5a08538e494fecc23046aab9a497af3704f4afdae3250ff").into(),
            },
            transactions: Some([bytes!("7ef8f8a0dc19cfa777d90980e4875d0a548a881baaa3f83f14d1bc0d3038bc329350e54194deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e20000f424000000000000000000000000300000000670d6d890000000000000125000000000000000000000000000000000000000000000000000000000000000700000000000000000000000000000000000000000000000000000000000000014bf9181db6e381d4384bbf69c48b0ee0eed23c6ca26143c6d2544f9d39997a590000000000000000000000007f83d659683caf2767fd3c720981d51f5bc365bc")].into()),
            no_tx_pool: false,
            block_data_hint: Some(BlockDataHint{
                extra_data: bytes!("476574682f76312e302e302f6c696e75782f676f312e342e32"),
                state_root: b256!("0x000000000000000000000000000000000000000000000000000000000000dead"),
                coinbase: Some(address!("0x000000000000000000000000000000000000dead")),
                nonce: Some(u64::MAX),
                difficulty: U256::from(10)
            }),
            gas_limit: Some(10_000_000),
        };

        assert_eq!(
            expected,
            payload_id_scroll(
                &b256!("3533bf30edaf9505d0810bf475cbe4e5f4b9889904b9845e83efdeab4e92eb1e"),
                &attrs,
                EngineApiMessageVersion::V3 as u8
            )
        );
    }
}
