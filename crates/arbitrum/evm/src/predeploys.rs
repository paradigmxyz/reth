#![allow(unused)]

use alloy_primitives::{Address, Bytes, U256, B256};

pub struct PredeployCallContext {
    pub block_number: u64,
    pub block_hashes: alloc::vec::Vec<B256>,
    pub chain_id: U256,
    pub os_version: u64,
    pub time: u64,
    pub origin: Address,
    pub caller: Address,
    pub depth: u64,
}

pub trait PredeployHandler {
    fn address(&self) -> Address;
    fn call(&self, ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, value: U256) -> (Bytes, u64, bool);
}

pub struct PredeployRegistry {
    handlers: alloc::vec::Vec<alloc::boxed::Box<dyn PredeployHandler + Send + Sync>>,
}

impl PredeployRegistry {
    pub fn new() -> Self {
        Self { handlers: alloc::vec::Vec::new() }
    }

    pub fn register(&mut self, handler: alloc::boxed::Box<dyn PredeployHandler + Send + Sync>) {
        self.handlers.push(handler);
    }

    pub fn dispatch(&self, ctx: &PredeployCallContext, to: Address, input: &Bytes, gas_limit: u64, value: U256) -> Option<(Bytes, u64, bool)> {
        for h in &self.handlers {
            if h.address() == to {
                return Some(h.call(ctx, input, gas_limit, value));
            }
        }
        None
    }



}

/* Minimal handler stubs; logic to be filled to match Nitro precompiles */

#[derive(Clone)]
pub struct ArbSys {
    pub addr: Address,
}

impl ArbSys {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbSys {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let send_tx_to_l1 = pre::selector(pre::SIG_SEND_TX_TO_L1);
        let withdraw_eth = pre::selector(pre::SIG_WITHDRAW_ETH);
        let create_retryable = pre::selector(pre::SIG_CREATE_RETRYABLE_TICKET);
        let redeem = pre::selector(pre::SIG_RETRY_REDEEM);
        let cancel = pre::selector(pre::SIG_CANCEL_RETRYABLE_TICKET);
        let arb_block_number = pre::selector(pre::SIG_ARB_BLOCK_NUMBER);
        let arb_block_hash = pre::selector(pre::SIG_ARB_BLOCK_HASH);
        let get_tx_call_value = pre::selector(pre::SIG_GET_TX_CALL_VALUE);
        let get_tx_origin = pre::selector(pre::SIG_GET_TX_ORIGIN);
        let get_block_number = pre::selector(pre::SIG_GET_BLOCK_NUMBER);
        let get_block_hash = pre::selector(pre::SIG_GET_BLOCK_HASH);
        let get_storage_at = pre::selector(pre::SIG_GET_STORAGE_AT);
        let arb_chain_id = pre::selector(pre::SIG_ARB_CHAIN_ID);
        let arb_os_version = pre::selector(pre::SIG_ARB_OS_VERSION);

        fn encode_u256(x: U256) -> Bytes {
            let mut out = [0u8; 32];
            x.to_be_bytes(&mut out);
            Bytes::from(out.to_vec())
        }
        fn encode_b256(x: B256) -> Bytes {
            Bytes::from(x.0.to_vec())
        }

        match sel {
            s if s == arb_block_number => {
                let out = encode_u256(U256::from(ctx.block_number));
                (out, gas_limit, true)
            }
            s if s == arb_block_hash => {
                if input.len() >= 4 + 32 {
                    let mut bn_bytes = [0u8; 32];
                    bn_bytes.copy_from_slice(&input[4..36]);
                    let bn = U256::from_be_bytes(bn_bytes);
                    let bn_u64 = bn.try_into().unwrap_or(0u64);
                    let current = ctx.block_number;
                    let ok_range = bn_u64 < current && current - bn_u64 <= 256;
                    let h = if ok_range {
                        let idx = (current - 1 - bn_u64) as usize;
                        ctx.block_hashes.get(idx).cloned().unwrap_or(B256::ZERO)
                    } else {
                        B256::ZERO
                    };
                    (encode_b256(h), gas_limit, true)
                } else {
                    (encode_b256(B256::ZERO), gas_limit, true)
                }
            }
            s if s == get_block_number => {
                let out = encode_u256(U256::from(ctx.block_number));
                (out, gas_limit, true)
            }
            s if s == get_block_hash => {
                if input.len() >= 4 + 32 {
                    let mut bn_bytes = [0u8; 32];
                    bn_bytes.copy_from_slice(&input[4..36]);
                    let bn = U256::from_be_bytes(bn_bytes);
                    let bn_u64 = bn.try_into().unwrap_or(0u64);
                    let current = ctx.block_number;
                    let ok_range = bn_u64 < current && current - bn_u64 <= 256;
                    let h = if ok_range {
                        let idx = (current - 1 - bn_u64) as usize;
                        ctx.block_hashes.get(idx).cloned().unwrap_or(B256::ZERO)
                    } else {
                        B256::ZERO
                    };
                    (encode_b256(h), gas_limit, true)
                } else {
                    (encode_b256(B256::ZERO), gas_limit, true)
                }
            }
            s if s == get_tx_origin => {
                let mut out = [0u8; 32];
                out[12..].copy_from_slice(ctx.origin.as_slice());
                (Bytes::from(out.to_vec()), gas_limit, true)
            }
            s if s == get_tx_call_value => {
                (encode_u256(U256::ZERO), gas_limit, true)
            }
            s if s == arb_chain_id => {
                let out = encode_u256(ctx.chain_id);
                (out, gas_limit, true)
            }
            s if s == arb_os_version => {
                let base = 56u64;
                let out = encode_u256(U256::from(base.saturating_add(ctx.os_version)));
                (out, gas_limit, true)
            }

            s if s == send_tx_to_l1 => (Bytes::default(), gas_limit, true),
            s if s == withdraw_eth => (Bytes::default(), gas_limit, true),
            s if s == create_retryable => (Bytes::default(), gas_limit, true),
            s if s == redeem => (Bytes::default(), gas_limit, true),
            s if s == cancel => (Bytes::default(), gas_limit, true),
            s if s == get_storage_at => (Bytes::default(), gas_limit, true),
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}

#[derive(Clone)]
pub struct ArbRetryableTx {
    pub addr: Address,
}

impl ArbRetryableTx {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbRetryableTx {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, _ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let redeem = pre::selector(pre::SIG_RETRY_REDEEM);
        let cancel = pre::selector(pre::SIG_CANCEL_RETRYABLE_TICKET);
        match sel {
            s if s == redeem => (Bytes::default(), gas_limit, true),
            s if s == cancel => (Bytes::default(), gas_limit, true),
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}

#[derive(Clone)]
pub struct ArbOwner {
    pub addr: Address,
}

impl ArbOwner {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbOwner {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, _ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        let _sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        (Bytes::default(), gas_limit, true)
    }
}

#[derive(Clone)]
pub struct ArbAddressTable {
    pub addr: Address,
}

impl ArbAddressTable {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

impl PredeployHandler for ArbAddressTable {
    fn address(&self) -> Address {
        self.addr
    }

    fn call(&self, _ctx: &PredeployCallContext, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let at_exists = pre::selector(pre::SIG_AT_ADDRESS_EXISTS);
        let at_compress = pre::selector(pre::SIG_AT_COMPRESS);
        let at_decompress = pre::selector(pre::SIG_AT_DECOMPRESS);
        let at_lookup = pre::selector(pre::SIG_AT_LOOKUP);
        let at_lookup_index = pre::selector(pre::SIG_AT_LOOKUP_INDEX);
        let at_register = pre::selector(pre::SIG_AT_REGISTER);
        let at_size = pre::selector(pre::SIG_AT_SIZE);
        match sel {
            s if s == at_exists => (Bytes::default(), gas_limit, true),
            s if s == at_compress => (Bytes::default(), gas_limit, true),
            s if s == at_decompress => (Bytes::default(), gas_limit, true),
            s if s == at_lookup => (Bytes::default(), gas_limit, true),
            s if s == at_lookup_index => (Bytes::default(), gas_limit, true),
            s if s == at_register => (Bytes::default(), gas_limit, true),
            s if s == at_size => (Bytes::default(), gas_limit, true),
            _ => (Bytes::default(), gas_limit, true),
        }
    }
}

impl PredeployRegistry {
    pub fn with_addresses(arb_sys: Address, arb_retryable_tx: Address, arb_owner: Address, arb_address_table: Address) -> Self {
        let mut reg = Self::new();
        reg.register(alloc::boxed::Box::new(ArbSys::new(arb_sys)));
        reg.register(alloc::boxed::Box::new(ArbRetryableTx::new(arb_retryable_tx)));
        reg.register(alloc::boxed::Box::new(ArbOwner::new(arb_owner)));
        reg.register(alloc::boxed::Box::new(ArbAddressTable::new(arb_address_table)));
        reg
    }

    pub fn with_default_addresses() -> Self {
        use arb_alloy_predeploys as pre;
        let mut reg = Self::new();
        reg.register(alloc::boxed::Box::new(ArbSys::new(Address::from(pre::ARB_SYS))));
        reg.register(alloc::boxed::Box::new(ArbRetryableTx::new(Address::from(pre::ARB_RETRYABLE_TX))));
        reg.register(alloc::boxed::Box::new(ArbOwner::new(Address::from(pre::ARB_OWNER))));
        reg.register(alloc::boxed::Box::new(ArbAddressTable::new(Address::from(pre::ARB_ADDRESS_TABLE))));
        reg
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, Bytes, U256};

    fn mk_ctx() -> PredeployCallContext {
        PredeployCallContext {
            block_number: 100,
            block_hashes: alloc::vec::Vec::new(),
            chain_id: U256::from(42161u64),
            os_version: 0,
            time: 0,
            origin: Address::ZERO,
            caller: Address::ZERO,
            depth: 1,
        }
    }

    fn mk_bytes() -> Bytes {
        Bytes::default()
    }

    #[test]
    fn dispatch_routes_to_correct_handler() {
        let a_sys = address!("0000000000000000000000000000000000000064");
        let a_retry = address!("000000000000000000000000000000000000006e");
        let a_owner = address!("0000000000000000000000000000000000000070");
        let a_table = address!("0000000000000000000000000000000000000066");

        let reg = PredeployRegistry::with_addresses(a_sys, a_retry, a_owner, a_table);
        let ctx = mk_ctx();

        let out_sys = reg.dispatch(&ctx, a_sys, &mk_bytes(), 100_000, U256::ZERO);
        assert!(out_sys.is_some());
        let out_retry_at_sys = reg.dispatch(&ctx, a_sys, &mk_bytes(), 100_000, U256::ZERO);
        assert!(out_retry_at_sys.is_some()); // same result as first since dispatch matches address only

        for addr in [a_retry, a_owner, a_table] {
            let out = reg.dispatch(&ctx, addr, &mk_bytes(), 42_000, U256::from(1));
            assert!(out.is_some());
        }

        let unknown = address!("00000000000000000000000000000000000000ff");
        assert!(reg.dispatch(&ctx, unknown, &mk_bytes(), 1, U256::ZERO).is_none());
    }
}
