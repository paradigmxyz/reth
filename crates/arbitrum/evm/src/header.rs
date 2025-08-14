use alloy_primitives::{B256, Bytes};
use alloy_consensus::Header;

#[derive(Clone, Debug, Default)]
pub struct ArbHeaderInfo {
    pub send_root: B256,
    pub send_count: u64,
    pub l1_block_number: u64,
    pub arbos_format_version: u64,
}

impl ArbHeaderInfo {
    pub fn apply_to_header(&self, header: &mut Header) {
        header.extra_data = Bytes::from(self.send_root.0.to_vec());

        let mut mix = [0u8; 32];
        mix[0..8].copy_from_slice(&self.send_count.to_be_bytes());
        mix[8..16].copy_from_slice(&self.l1_block_number.to_be_bytes());
        mix[16..24].copy_from_slice(&self.arbos_format_version.to_be_bytes());
        header.mix_hash = B256::from(mix);
    }
}

pub fn derive_arb_header_info_from_state<
    F: for<'a> alloy_evm::block::BlockExecutorFactory,
>(
    _input: &reth_evm::execute::BlockAssemblerInput<'_, '_, F, Header>,
) -> Option<ArbHeaderInfo> {
    None
}
