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
