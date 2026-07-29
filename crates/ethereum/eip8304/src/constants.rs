use alloy_primitives::{address, hex, Address, Bytes};

pub const TABLE_SIZES: [u64; 5] = [1, 4, 16, 64, 256];
pub const TABLES_PER_LEVEL: u64 = 1024;

pub const ENTRY_TYPE_BLOCK: u8 = 0;
pub const ENTRY_TYPE_TRANSACTION: u8 = 1;
pub const ENTRY_TYPE_LOG_ADDRESS: u8 = 2;
pub const ENTRY_TYPE_LOG_TOPIC0: u8 = 3;
pub const ENTRY_TYPE_LOG_TOPIC1: u8 = 4;
pub const ENTRY_TYPE_LOG_TOPIC2: u8 = 5;
pub const ENTRY_TYPE_LOG_TOPIC3: u8 = 6;

pub const SYSTEM_CALL_GAS_LIMIT: u64 = 30_000_000;

/// Placeholder address for the index contract.
/// The EIP marks `INDEX_CONTRACT_ADDRESS` as `<TBD>`; the final address will be
/// derived from a synthetic deployment transaction chosen by the EIP authors.
/// Uses a synthetic value in the system-contract range (mnemonic `0x8304`) so it
/// cannot collide with the precompiles (`0x01`..`0x0a`) once integration tests
/// begin executing the contract.
/// TODO: update when EIP-8304 INDEX_CONTRACT_ADDRESS is finalized
pub const INDEX_CONTRACT_ADDRESS: Address = address!("0000000000000000000000000000000000008304");

/// The deployment transaction input (init code) of the index contract, verbatim
/// from the EIP-8304 specification. The first 9 bytes (`60758060095f395ff3`) are
/// the constructor that returns the runtime; the remainder is [`INDEX_CONTRACT_CODE`].
pub const INDEX_CONTRACT_INIT_CODE: &[u8] = &hex!(
    "60758060095f395ff33373fffffffffffffffffffffffffffffffffffffffe1460605760403603605c576020358060801c605c576104008160048104430304828202925f35818106605c5704908103196103ff10605c570601548015605c575f5260205ff35b5f5ffd5b604035602035610400818102915f350406015500"
);

/// The runtime bytecode of the index contract, from the EIP-8304 specification.
/// Handles both `get` (any caller) and `set` (SYSTEM_ADDRESS only) operations.
///
/// Kept as a hex string (not a hand-typed byte array) so it is directly verifiable
/// against the EIP; the `constants` tests additionally assert it equals
/// `INDEX_CONTRACT_INIT_CODE` with the 9-byte constructor stripped.
pub const INDEX_CONTRACT_CODE: Bytes = Bytes::from_static(&hex!(
    "3373fffffffffffffffffffffffffffffffffffffffe1460605760403603605c576020358060801c605c576104008160048104430304828202925f35818106605c5704908103196103ff10605c570601548015605c575f5260205ff35b5f5ffd5b604035602035610400818102915f350406015500"
));

/// The number of leading bytes in [`INDEX_CONTRACT_INIT_CODE`] that make up the
/// constructor (`60758060095f395ff3`), stripped to obtain the runtime code.
#[cfg(test)]
const INIT_CODE_CONSTRUCTOR_LEN: usize = 9;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_code_is_117_bytes() {
        // EIP constructor `PUSH1 0x75` returns 0x75 = 117 runtime bytes.
        assert_eq!(INDEX_CONTRACT_CODE.len(), 117);
    }

    #[test]
    fn runtime_code_is_init_code_without_constructor() {
        assert_eq!(
            INDEX_CONTRACT_CODE.as_ref(),
            &INDEX_CONTRACT_INIT_CODE[INIT_CODE_CONSTRUCTOR_LEN..]
        );
    }

    #[test]
    fn runtime_code_has_valid_system_address_guard() {
        // Guards the exact prologue that a hand-typed transcription previously
        // corrupted: CALLER, PUSH20, then the 20-byte SYSTEM_ADDRESS operand
        // (nineteen 0xff bytes followed by 0xfe).
        let code = INDEX_CONTRACT_CODE;
        assert_eq!(code[0], 0x33, "expected CALLER");
        assert_eq!(code[1], 0x73, "expected PUSH20");
        assert_eq!(&code[2..21], &[0xff; 19]);
        assert_eq!(code[21], 0xfe);
    }

    #[test]
    fn contract_address_is_not_a_precompile() {
        // Precompiles occupy 0x01..=0x0a; the placeholder must not collide.
        let last = INDEX_CONTRACT_ADDRESS.as_slice()[19];
        let is_low = INDEX_CONTRACT_ADDRESS.as_slice()[..19].iter().all(|b| *b == 0);
        assert!(!(is_low && (1..=0x0a).contains(&last)));
    }
}
