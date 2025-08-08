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

    fn call(&self, _input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        (Bytes::default(), gas_limit, true)
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

    fn call(&self, _input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        (Bytes::default(), gas_limit, true)
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

    fn call(&self, _input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
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

    fn call(&self, _input: &Bytes, gas_limit: u64, _value: U256) -> (Bytes, u64, bool) {
        (Bytes::default(), gas_limit, true)
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
impl PredeployRegistry {
    pub fn with_default_addresses() -> Self {
        use arb_alloy_predeploys as pre;
        let mut reg = Self::new();
        reg.register(alloc::boxed::Box::new(ArbSys::new(pre::addresses::ARB_SYS)));
        reg.register(alloc::boxed::Box::new(ArbRetryableTx::new(pre::addresses::ARB_RETRYABLE_TX)));
        reg.register(alloc::boxed::Box::new(ArbOwner::new(pre::addresses::ARB_OWNER)));
        reg.register(alloc::boxed::Box::new(ArbAddressTable::new(pre::addresses::ARB_ADDRESS_TABLE)));
        reg
    }
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
