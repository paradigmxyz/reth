use crate::primitives::CustomTransaction;
use alloy_consensus::{
    crypto::RecoveryError,
    transaction::{Recovered, SignerRecoverable},
};
use alloy_eips::{eip2718::WithEncoded, Decodable2718};
use alloy_primitives::B256;
use alloy_rpc_types_engine::PayloadId;
use op_alloy_rpc_types_engine::{
    OpFlashblockPayloadBase, OpFlashblockPayloadDelta, OpFlashblockPayloadMetadata,
};
use reth_optimism_flashblocks::{FlashblockPayload, FlashblockPayloadBase};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomFlashblockPayloadBase {
    #[serde(flatten)]
    pub inner: OpFlashblockPayloadBase,
    pub extension: u64,
}

impl FlashblockPayloadBase for CustomFlashblockPayloadBase {
    fn parent_hash(&self) -> B256 {
        self.inner.parent_hash
    }

    fn block_number(&self) -> u64 {
        self.inner.block_number
    }

    fn timestamp(&self) -> u64 {
        self.inner.timestamp
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFlashblockPayload {
    pub payload_id: PayloadId,
    pub index: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<CustomFlashblockPayloadBase>,
    pub diff: OpFlashblockPayloadDelta,
    pub metadata: OpFlashblockPayloadMetadata,
}

impl FlashblockPayload for CustomFlashblockPayload {
    type Base = CustomFlashblockPayloadBase;
    type Diff = OpFlashblockPayloadDelta;
    type SignedTx = CustomTransaction;
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
        self.metadata.block_number
    }

    fn recover_transactions(
        &self,
    ) -> impl Iterator<Item = Result<WithEncoded<Recovered<Self::SignedTx>>, RecoveryError>> {
        self.diff.transactions.clone().into_iter().map(|raw| {
            let tx = CustomTransaction::decode_2718(&mut raw.as_ref())
                .map_err(RecoveryError::from_source)?;
            tx.try_into_recovered().map(|tx| tx.into_encoded_with(raw.clone()))
        })
    }
}
