//! Helpers for deriving contract addresses

// re-export from revm
use crate::{keccak256, Address, U256};
pub use revm_primitives::utilities::{create2_address, create_address};

/// Returns the CREATE2 address of a smart contract as specified in
/// [EIP1014](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1014.md)
///
/// keccak256( 0xff ++ senderAddress ++ salt ++ keccak256(init_code))[12..]
///
/// where `salt` is always 32 bytes (a stack item).
pub fn create2_address_from_code(
    from: Address,
    init_code: impl AsRef<[u8]>,
    salt: U256,
) -> Address {
    let init_code_hash = keccak256(init_code);
    create2_address(from, init_code_hash, salt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hex;

    #[test]
    fn contract_address() {
        // http://ethereum.stackexchange.com/questions/760/how-is-the-address-of-an-ethereum-contract-computed
        let from = "6ac7ea33f8831ea9dcc53393aaa88b25a785dbf0".parse::<Address>().unwrap();
        for (nonce, expected) in [
            "cd234a471b72ba2f1ccf0a70fcaba648a5eecd8d",
            "343c43a37d37dff08ae8c4a11544c718abb4fcf8",
            "f778b86fa74e846c4f0a1fbd1335fe81c00a0c91",
            "fffd933a0bc612844eaf0c6fe3e5b8e9b6c1d19c",
        ]
        .iter()
        .enumerate()
        {
            let address = create_address(from, nonce as u64);
            assert_eq!(address, expected.parse::<Address>().unwrap());
        }
    }

    // Test vectors from https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1014.md#examples
    #[test]
    fn test_create2_address() {
        for (from, salt, init_code, expected) in &[
            (
                "0000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "00",
                "4D1A2e2bB4F88F0250f26Ffff098B0b30B26BF38",
            ),
            (
                "deadbeef00000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "00",
                "B928f69Bb1D91Cd65274e3c79d8986362984fDA3",
            ),
            (
                "deadbeef00000000000000000000000000000000",
                "000000000000000000000000feed000000000000000000000000000000000000",
                "00",
                "D04116cDd17beBE565EB2422F2497E06cC1C9833",
            ),
            (
                "0000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "deadbeef",
                "70f2b2914A2a4b783FaEFb75f459A580616Fcb5e",
            ),
            (
                "00000000000000000000000000000000deadbeef",
                "00000000000000000000000000000000000000000000000000000000cafebabe",
                "deadbeef",
                "60f3f640a8508fC6a86d45DF051962668E1e8AC7",
            ),
            (
                "00000000000000000000000000000000deadbeef",
                "00000000000000000000000000000000000000000000000000000000cafebabe",
                "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                "1d8bfDC5D46DC4f61D6b6115972536eBE6A8854C",
            ),
            (
                "0000000000000000000000000000000000000000",
                "0000000000000000000000000000000000000000000000000000000000000000",
                "",
                "E33C0C7F7df4809055C3ebA6c09CFe4BaF1BD9e0",
            ),
        ] {
            // create2_address()
            let from = from.parse::<Address>().unwrap();
            let salt = hex::decode(salt).unwrap();
            let salt = U256::try_from_be_slice(&salt).unwrap();
            let init_code = hex::decode(init_code).unwrap();
            let expected = expected.parse::<Address>().unwrap();
            assert_eq!(expected, create2_address_from_code(from, init_code.clone(), salt));

            // get_create2_address_from_hash()
            let init_code_hash = keccak256(init_code);
            assert_eq!(expected, create2_address(from, init_code_hash,  salt))
        }
    }
}
