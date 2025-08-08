#![allow(unused)]

use alloy_primitives::{Address, Bytes, U256};

pub trait PredeployHandler {
    fn address(&self) -> Address;
    fn call(&self, input: &Bytes, gas_limit: u64, value: U256) -> (Bytes, u64, bool);
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

    pub fn dispatch(&self, to: Address, input: &Bytes, gas_limit: u64, value: U256) -> Option<(Bytes, u64, bool)> {
        for h in &self.handlers {
            if h.address() == to {
                return Some(h.call(input, gas_limit, value));
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

    fn call(&self, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let _send_tx_to_l1 = pre::selector(pre::SIG_SEND_TX_TO_L1);
        let _withdraw_eth = pre::selector(pre::SIG_WITHDRAW_ETH);
        let _create_retryable = pre::selector(pre::SIG_CREATE_RETRYABLE_TICKET);
        let _redeem = pre::selector(pre::SIG_RETRY_REDEEM);
        let _cancel = pre::selector(pre::SIG_CANCEL_RETRYABLE_TICKET);
        let _block_number = pre::selector(pre::SIG_ARB_BLOCK_NUMBER);
        let _block_hash = pre::selector(pre::SIG_ARB_BLOCK_HASH);
        let _get_tx_call_value = pre::selector(pre::SIG_GET_TX_CALL_VALUE);
        let _get_tx_origin = pre::selector(pre::SIG_GET_TX_ORIGIN);
        let _get_block_number = pre::selector(pre::SIG_GET_BLOCK_NUMBER);
        let _get_block_hash = pre::selector(pre::SIG_GET_BLOCK_HASH);
        let _get_storage_at = pre::selector(pre::SIG_GET_STORAGE_AT);
        match sel {
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

    fn call(&self, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let _redeem = pre::selector(pre::SIG_RETRY_REDEEM);
        let _cancel = pre::selector(pre::SIG_CANCEL_RETRYABLE_TICKET);
        match sel {
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

    fn call(&self, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
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

    fn call(&self, input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        use arb_alloy_predeploys as pre;
        let sel = input.get(0..4).map(|s| [s[0], s[1], s[2], s[3]]).unwrap_or([0u8; 4]);
        let _at_exists = pre::selector(pre::SIG_AT_ADDRESS_EXISTS);
        let _at_compress = pre::selector(pre::SIG_AT_COMPRESS);
        let _at_decompress = pre::selector(pre::SIG_AT_DECOMPRESS);
        let _at_lookup = pre::selector(pre::SIG_AT_LOOKUP);
        let _at_lookup_index = pre::selector(pre::SIG_AT_LOOKUP_INDEX);
        let _at_register = pre::selector(pre::SIG_AT_REGISTER);
        let _at_size = pre::selector(pre::SIG_AT_SIZE);
        match sel {
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

        let out_sys = reg.dispatch(a_sys, &mk_bytes(), 100_000, U256::ZERO);
        assert!(out_sys.is_some());
        let out_retry_at_sys = reg.dispatch(a_sys, &mk_bytes(), 100_000, U256::ZERO);
        assert!(out_retry_at_sys.is_some()); // same result as first since dispatch matches address only

        for addr in [a_retry, a_owner, a_table] {
            let out = reg.dispatch(addr, &mk_bytes(), 42_000, U256::from(1));
            assert!(out.is_some());
        }

        let unknown = address!("00000000000000000000000000000000000000ff");
        assert!(reg.dispatch(unknown, &mk_bytes(), 1, U256::ZERO).is_none());
    }
}
