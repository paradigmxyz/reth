use crate::primitives::CustomTransaction;
use alloy_consensus::{crypto::RecoveryError, transaction::Recovered};
use alloy_eips::{eip2718::WithEncoded, eip4895::Withdrawals};
use alloy_primitives::{Bloom, Bytes, B256};
use alloy_rpc_types_engine::PayloadId;
use reth_optimism_flashblocks::{FlashblockDiff, FlashblockPayload, FlashblockPayloadBase};
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, Default)]
pub struct CustomFlashblockPayloadBase {
    pub parent_hash: B256,
    pub block_number: u64,
    pub timestamp: u64,
}

impl FlashblockPayloadBase for CustomFlashblockPayloadBase {
    fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    fn block_number(&self) -> u64 {
        self.block_number
    }

    fn timestamp(&self) -> u64 {
        self.timestamp
    }
}

#[derive(Debug, Clone, Default)]
pub struct CustomFlashblockPayloadDiff {
    pub block_hash: B256,
    pub state_root: B256,
    pub gas_used: u64,
    pub logs_bloom: Bloom,
    pub receipts_root: B256,
    pub transactions: Vec<Bytes>,
}

impl FlashblockDiff for CustomFlashblockPayloadDiff {
    fn block_hash(&self) -> B256 {
        self.block_hash
    }

    fn state_root(&self) -> B256 {
        self.state_root
    }

    fn gas_used(&self) -> u64 {
        self.gas_used
    }

    fn logs_bloom(&self) -> &Bloom {
        &self.logs_bloom
    }

    fn receipts_root(&self) -> B256 {
        self.receipts_root
    }

    fn transactions_raw(&self) -> &[Bytes] {
        &self.transactions
    }

    fn withdrawals(&self) -> Option<&Withdrawals> {
        None
    }

    fn withdrawals_root(&self) -> Option<B256> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct CustomFlashblockPayload {
    pub index: u64,
    pub payload_id: PayloadId,
    pub base: Option<CustomFlashblockPayloadBase>,
    pub diff: CustomFlashblockPayloadDiff,
}

impl<'de> Deserialize<'de> for CustomFlashblockPayload {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        todo!("implement deserialization")
    }
}

impl FlashblockPayload for CustomFlashblockPayload {
    type Base = CustomFlashblockPayloadBase;
    type Diff = CustomFlashblockPayloadDiff;
    type SignedTx = CustomTransaction;

    fn index(&self) -> u64 {
        self.index
    }

    fn payload_id(&self) -> PayloadId {
        self.payload_id
    }

    fn base(&self) -> Option<&Self::Base> {
        self.base.as_ref()
    }

    fn diff(&self) -> &Self::Diff {
        &self.diff
    }

    fn block_number(&self) -> u64 {
        self.base.as_ref().map(|b| b.block_number()).unwrap_or(0)
    }

    fn recover_transactions(
        &self,
    ) -> impl Iterator<Item = Result<WithEncoded<Recovered<Self::SignedTx>>, RecoveryError>> {
        std::iter::from_fn(|| todo!("implement transaction recovery"))
    }
}
