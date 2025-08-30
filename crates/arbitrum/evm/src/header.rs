use alloy_consensus::Header;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};

pub fn compute_nitro_mixhash(send_count: u64, l1_block_number: u64, arbos_version: u64) -> B256 {
    let mut mix = [0u8; 32];
    mix[0..8].copy_from_slice(&send_count.to_be_bytes());
    mix[8..16].copy_from_slice(&l1_block_number.to_be_bytes());
    mix[16..24].copy_from_slice(&arbos_version.to_be_bytes());
    B256::from(mix)
}

pub fn extract_send_root_from_header_extra(extra: &[u8]) -> B256 {
    if extra.len() >= 32 {
        B256::from_slice(&extra[..32])
    } else {
        B256::ZERO
    }
}
 
use reth_storage_api::StateProvider;

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

const fn arbos_state_address() -> Address {
    alloy_primitives::address!("0xA4B05FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF")
}

fn uint_to_hash_u64_be(k: u64) -> B256 {
    let mut out = [0u8; 32];
    out[24..32].copy_from_slice(&k.to_be_bytes());
    B256::from(out)
}

fn storage_key_map(storage_key: &[u8], key: B256) -> B256 {
    let boundary = 31usize;
    let mut data = Vec::with_capacity(storage_key.len() + boundary);
    data.extend_from_slice(storage_key);
    data.extend_from_slice(&key.0[..boundary]);
    let h = keccak256(&data);
    let mut mapped = [0u8; 32];
    mapped[..boundary].copy_from_slice(&h.0[..boundary]);
    mapped[boundary] = key.0[boundary];
    B256::from(mapped)
}

fn subspace(parent: &[u8], id: &[u8]) -> [u8; 32] {
    let mut data = Vec::with_capacity(parent.len() + id.len());
    data.extend_from_slice(parent);
    data.extend_from_slice(id);
    keccak256(&data).0
}

fn read_storage_u64_be(provider: &dyn StateProvider, addr: Address, slot: B256) -> Option<u64> {
    let val: U256 = provider.storage(addr, slot).ok()??;
    let bytes: [u8; 32] = val.to_be_bytes::<32>();
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[24..32]);
    Some(u64::from_be_bytes(buf))
}

fn read_storage_hash(provider: &dyn StateProvider, addr: Address, slot: B256) -> Option<B256> {
    let val: U256 = provider.storage(addr, slot).ok()??;
    let bytes: [u8; 32] = val.to_be_bytes::<32>();
    Some(B256::from(bytes))
}

fn calc_num_partials(size: u64) -> u64 {
    if size == 0 {
        return 0;
    }
    let mut p = 0u64;
    let mut v = size - 1;
    while v > 0 {
        v >>= 1;
        p += 1;
    }
    p
}

fn merkle_root_from_partials(
    provider: &dyn StateProvider,
    addr: Address,
    send_merkle_storage_key: &[u8],
    size: u64,
) -> Option<B256> {
    if size == 0 {
        return Some(B256::ZERO);
    }
    let mut hash_so_far: Option<B256> = None;
    let mut capacity_in_hash: u64 = 0;
    let mut capacity = 1u64;
    let num_partials = calc_num_partials(size);
    for level in 0..num_partials {
        let key = uint_to_hash_u64_be(2 + level);
        let slot = storage_key_map(send_merkle_storage_key, key);
        let partial = read_storage_hash(provider, addr, slot).unwrap_or(B256::ZERO);
        if partial != B256::ZERO {
            if let Some(mut h) = hash_so_far {
                while capacity_in_hash < capacity {
                    let x = keccak256(&[h.0.as_slice(), &[0u8; 32]].concat());
                    h = B256::from(x.0);
                    capacity_in_hash *= 2;
                }
                let x = keccak256(&[partial.0.as_slice(), h.0.as_slice()].concat());
                hash_so_far = Some(B256::from(x.0));
                capacity_in_hash = 2 * capacity;
            } else {
                hash_so_far = Some(partial);
                capacity_in_hash = capacity;
            }
        }
        capacity = capacity.saturating_mul(2);
    }
    hash_so_far
}

pub fn derive_arb_header_info_from_state<F: for<'a> alloy_evm::block::BlockExecutorFactory>(
    input: &reth_evm::execute::BlockAssemblerInput<'_, '_, F, Header>,
) -> Option<ArbHeaderInfo> {
    let addr = arbos_state_address();
    let root_storage_key: &[u8] = &[];

    let version_slot = storage_key_map(&root_storage_key, uint_to_hash_u64_be(0));
    let arbos_version = {
        if let Some(acc) = input.bundle_state.account(&addr) {
            if let Some(ver_u256) = acc.storage_slot(alloy_primitives::U256::from_be_bytes(version_slot.0)) {
                let bytes: [u8; 32] = ver_u256.to_be_bytes::<32>();
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&bytes[24..32]);
                u64::from_be_bytes(buf)
            } else {
                read_storage_u64_be(input.state_provider, addr, version_slot)?
            }
        } else {
            read_storage_u64_be(input.state_provider, addr, version_slot)?
        }
    };

    let send_merkle_sub = subspace(&root_storage_key, &[5u8]);
    let blockhashes_sub = subspace(&root_storage_key, &[6u8]);

    let send_count_slot = storage_key_map(&send_merkle_sub, uint_to_hash_u64_be(0));
    let send_count = if let Some(acc) = input.bundle_state.account(&addr) {
        if let Some(sc_u256) = acc.storage_slot(alloy_primitives::U256::from_be_bytes(send_count_slot.0)) {
            let bytes: [u8; 32] = sc_u256.to_be_bytes::<32>();
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[24..32]);
            u64::from_be_bytes(buf)
        } else {
            read_storage_u64_be(input.state_provider, addr, send_count_slot).unwrap_or(0)
        }
    } else {
        read_storage_u64_be(input.state_provider, addr, send_count_slot).unwrap_or(0)
    };

    let send_root = if let Some(acc) = input.bundle_state.account(&addr) {
        merkle_root_from_partials(input.state_provider, addr, &send_merkle_sub, send_count)
            .unwrap_or(B256::ZERO)
    } else {
        merkle_root_from_partials(input.state_provider, addr, &send_merkle_sub, send_count)
            .unwrap_or(B256::ZERO)
    };

    let l1_block_num_slot = storage_key_map(&blockhashes_sub, uint_to_hash_u64_be(0));
    let l1_block_number = if let Some(acc) = input.bundle_state.account(&addr) {
        if let Some(bn_u256) = acc.storage_slot(alloy_primitives::U256::from_be_bytes(l1_block_num_slot.0)) {
            let bytes: [u8; 32] = bn_u256.to_be_bytes::<32>();
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes[24..32]);
            u64::from_be_bytes(buf)
        } else {
            read_storage_u64_be(input.state_provider, addr, l1_block_num_slot).unwrap_or(0)
        }
    } else {
        read_storage_u64_be(input.state_provider, addr, l1_block_num_slot).unwrap_or(0)
    };

    Some(ArbHeaderInfo {
        send_root,
        send_count,
        l1_block_number,
        arbos_format_version: arbos_version,
    })
}

pub fn arbos_l1_block_number_slot() -> (Address, B256) {
    let addr = arbos_state_address();
    let root_storage_key: &[u8] = &[];
    let blockhashes_sub = subspace(&root_storage_key, &[6u8]);
    let l1_block_num_slot = storage_key_map(&blockhashes_sub, uint_to_hash_u64_be(0));
    (addr, l1_block_num_slot)
}
pub fn read_arbos_version(provider: &dyn StateProvider) -> Option<u64> {
    let addr = arbos_state_address();
    let root_storage_key: &[u8] = &[];
    let version_slot = storage_key_map(&root_storage_key, uint_to_hash_u64_be(0));
    read_storage_u64_be(provider, addr, version_slot)
}

pub fn read_l2_per_block_gas_limit(provider: &dyn StateProvider) -> Option<u64> {
    let addr = arbos_state_address();
    let root_storage_key: &[u8] = &[];
    let l2_pricing_subspace = subspace(&root_storage_key, &[1u8]);
    let per_block_gas_limit_slot = storage_key_map(&l2_pricing_subspace, uint_to_hash_u64_be(1));
    read_storage_u64_be(provider, addr, per_block_gas_limit_slot)
}
pub fn read_l2_base_fee(provider: &dyn StateProvider) -> Option<u64> {
    let addr = arbos_state_address();
    let root_storage_key: &[u8] = &[];
    let l2_pricing_subspace = subspace(&root_storage_key, &[1u8]);
    let price_per_unit_slot = storage_key_map(&l2_pricing_subspace, uint_to_hash_u64_be(2));
    read_storage_u64_be(provider, addr, price_per_unit_slot)
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;

    #[test]
    fn apply_to_header_encodes_sendroot_and_mix_parts() {
        let send_root = B256::from_slice(&[0x11u8; 32]);
        let send_count: u64 = 0x0102030405060708;
        let l1_block_number: u64 = 0x1112131415161718;
        let arbos_format_version: u64 = 0x2122232425262728;

        let info = ArbHeaderInfo {
            send_root,
            send_count,
            l1_block_number,
            arbos_format_version,
        };

        let mut h = Header::default();
        info.apply_to_header(&mut h);

        assert_eq!(h.extra_data.len(), 32);
        assert_eq!(&h.extra_data[..], &send_root.0[..]);

        let mix = h.mix_hash;
        let mut expect = [0u8; 32];
        expect[0..8].copy_from_slice(&send_count.to_be_bytes());
        expect[8..16].copy_from_slice(&l1_block_number.to_be_bytes());
        expect[16..24].copy_from_slice(&arbos_format_version.to_be_bytes());
        assert_eq!(mix, B256::from(expect));
    }
}
