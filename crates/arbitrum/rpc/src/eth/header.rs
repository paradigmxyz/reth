use alloy_primitives::{B256, U256};
use alloy_rpc_types_eth::Header as RpcHeader;
use alloy_serde::WithOtherFields;
use reth_primitives_traits::{SealedHeader};
use reth_rpc_convert::transaction::HeaderConverter;
use reth_arbitrum_evm::header::extract_send_root_from_header_extra;

#[derive(Clone, Debug)]
pub struct ArbHeaderConverter;

impl HeaderConverter<alloy_consensus::Header, WithOtherFields<RpcHeader<alloy_consensus::Header>>> for ArbHeaderConverter {
    type Err = std::convert::Infallible;

    fn convert_header(&self, header: SealedHeader<alloy_consensus::Header>, block_size: usize) -> Result<WithOtherFields<RpcHeader<alloy_consensus::Header>>, Self::Err> {
        let base = RpcHeader::from_consensus(header.clone().into(), None, Some(U256::from(block_size)));
        let mut out = WithOtherFields::new(base);

        let h = header.header();
        let extra = h.extra_data.as_ref();
        let send_root = extract_send_root_from_header_extra(extra);
        let mix = h.mix_hash;

        let mut send_count_bytes = [0u8; 8];
        send_count_bytes.copy_from_slice(&mix.0[0..8]);
        let send_count = u64::from_be_bytes(send_count_bytes);

        let mut l1_bn_bytes = [0u8; 8];
        l1_bn_bytes.copy_from_slice(&mix.0[8..16]);
        let l1_block_number = u64::from_be_bytes(l1_bn_bytes);

        let _ = out.other.insert_value("sendRoot".to_string(), send_root);
        let _ = out.other.insert_value("sendCount".to_string(), U256::from(send_count));
        let _ = out.other.insert_value("l1BlockNumber".to_string(), U256::from(l1_block_number));

        Ok(out)
    }
}
