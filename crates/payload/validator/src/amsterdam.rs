//! Amsterdam rules for new payloads.

use alloy_consensus::BlockHeader;
use alloy_rpc_types_engine::PayloadError;
use reth_primitives_traits::{Block, SealedBlock};

/// Checks that Amsterdam header fields are present if Amsterdam is active and absent otherwise.
#[inline]
pub fn ensure_well_formed_fields<T: Block>(
    block: &SealedBlock<T>,
    is_amsterdam_active: bool,
) -> Result<(), PayloadError> {
    if is_amsterdam_active {
        if block.block_access_list_hash().is_none() {
            return Err(PayloadError::PostAmsterdamBlockWithoutBlockAccessList)
        }
        if block.slot_number().is_none() {
            return Err(PayloadError::PostAmsterdamBlockWithoutSlotNumber)
        }
    } else {
        if block.block_access_list_hash().is_some() {
            return Err(PayloadError::PreAmsterdamBlockWithBlockAccessList)
        }
        if block.slot_number().is_some() {
            return Err(PayloadError::PreAmsterdamBlockWithSlotNumber)
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Block as AlloyBlock, Header, TxEnvelope};

    fn block(
        has_block_access_list: bool,
        has_slot_number: bool,
    ) -> SealedBlock<AlloyBlock<TxEnvelope>> {
        let header = Header {
            block_access_list_hash: has_block_access_list.then_some(Default::default()),
            slot_number: has_slot_number.then_some(0),
            ..Default::default()
        };
        AlloyBlock { header, body: Default::default() }.seal_slow()
    }

    #[test]
    fn validates_amsterdam_fields() {
        assert!(ensure_well_formed_fields(&block(true, true), true).is_ok());
        assert!(ensure_well_formed_fields(&block(false, false), false).is_ok());

        assert!(matches!(
            ensure_well_formed_fields(&block(false, true), true),
            Err(PayloadError::PostAmsterdamBlockWithoutBlockAccessList)
        ));
        assert!(matches!(
            ensure_well_formed_fields(&block(true, false), true),
            Err(PayloadError::PostAmsterdamBlockWithoutSlotNumber)
        ));
        assert!(matches!(
            ensure_well_formed_fields(&block(true, false), false),
            Err(PayloadError::PreAmsterdamBlockWithBlockAccessList)
        ));
        assert!(matches!(
            ensure_well_formed_fields(&block(false, true), false),
            Err(PayloadError::PreAmsterdamBlockWithSlotNumber)
        ));
    }
}
