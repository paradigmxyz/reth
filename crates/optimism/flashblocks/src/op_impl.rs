//! Optimism implementation of flashblock traits.

use crate::traits::{FlashblockDiff, FlashblockMetadata, FlashblockPayload, FlashblockPayloadBase};
use alloy_consensus::crypto::RecoveryError;
use alloy_eips::eip2718::WithEncoded;
use alloy_primitives::{Bloom, Bytes, B256};
use alloy_rpc_types_engine::PayloadId;
use op_alloy_consensus::{OpReceipt, OpTxEnvelope};
use op_alloy_rpc_types_engine::{
    OpFlashblockPayload, OpFlashblockPayloadBase, OpFlashblockPayloadDelta,
    OpFlashblockPayloadMetadata,
};
use reth_primitives_traits::Recovered;

impl FlashblockPayloadBase for OpFlashblockPayloadBase {
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

impl FlashblockDiff for OpFlashblockPayloadDelta {
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
}

impl FlashblockMetadata for OpFlashblockPayloadMetadata {
    type Receipt = OpReceipt;

    fn receipts(&self) -> impl Iterator<Item = (B256, &Self::Receipt)> {
        self.receipts.iter().map(|(k, v)| (*k, v))
    }
}

impl FlashblockPayload for OpFlashblockPayload {
    type Base = OpFlashblockPayloadBase;
    type Diff = OpFlashblockPayloadDelta;
    type SignedTx = OpTxEnvelope;
    type Metadata = OpFlashblockPayloadMetadata;

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

    fn metadata(&self) -> &Self::Metadata {
        &self.metadata
    }

    fn block_number(&self) -> u64 {
        Self::block_number(self)
    }

    fn recover_transactions(
        &self,
    ) -> impl Iterator<Item = Result<WithEncoded<Recovered<Self::SignedTx>>, RecoveryError>> {
        Self::recover_transactions::<OpTxEnvelope>(self)
    }
}
