//! Optimism implementation of flashblock traits.

use crate::traits::{FlashblockDiff, FlashblockPayload, FlashblockPayloadBase};
use alloy_consensus::crypto::RecoveryError;
use alloy_eips::{eip2718::WithEncoded, eip4895::Withdrawals};
use alloy_primitives::{Bloom, Bytes, B256};
use alloy_rpc_types_engine::PayloadId;
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_rpc_types_engine::{
    OpFlashblockPayload, OpFlashblockPayloadBase, OpFlashblockPayloadDelta,
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

    fn withdrawals(&self) -> Option<&Withdrawals> {
        // TODO: Might not be needed as withdrawals aren't processed in a block except if at start
        // or end
        None
    }

    fn withdrawals_root(&self) -> Option<B256> {
        Some(self.withdrawals_root)
    }
}

impl FlashblockPayload for OpFlashblockPayload {
    type Base = OpFlashblockPayloadBase;
    type Diff = OpFlashblockPayloadDelta;
    type SignedTx = OpTxEnvelope;

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
        Self::block_number(self)
    }

    fn recover_transactions(
        &self,
    ) -> impl Iterator<Item = Result<WithEncoded<Recovered<Self::SignedTx>>, RecoveryError>> {
        Self::recover_transactions::<OpTxEnvelope>(self)
    }
}
