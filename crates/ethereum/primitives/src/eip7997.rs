//! Constants for [EIP-7997]: the deterministic factory predeploy.
//!
//! [EIP-7997]: https://eips.ethereum.org/EIPS/eip-7997

use alloy_primitives::{address, bytes, Address, Bytes};

/// Address of the EIP-7997 deterministic deployment proxy.
pub const FACTORY_ADDRESS: Address = address!("0x4e59b44847b379578588920cA78FbF26c0B4956C");

/// Runtime bytecode of the EIP-7997 deterministic deployment proxy.
pub const FACTORY_CODE: Bytes = bytes!(
    "0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf3"
);
