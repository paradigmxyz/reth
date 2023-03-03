//! Helpers for deriving contract addresses

use crate::{keccak256, Address, H256, U256};
use reth_rlp::Encodable;
use reth_rlp_derive::RlpEncodable;

/// The address for an Ethereum contract is deterministically computed from the
/// address of its creator (sender) and how many transactions the creator has
/// sent (nonce). The sender and nonce are RLP encoded and then hashed with Keccak-256.
pub fn get_contract_address(sender: impl Into<Address>, nonce: impl Into<U256>) -> Address {
    #[derive(RlpEncodable)]
    struct S {
        sender: Address,
        nonce: U256,
    }
    let sender = S { sender: sender.into(), nonce: nonce.into() };
    let mut buf = Vec::new();
    sender.encode(&mut buf);
    let hash = keccak256(buf);
    let addr: [u8; 20] = hash[12..].try_into().expect("correct len");
    Address::from(addr)
}

/// Returns the CREATE2 address of a smart contract as specified in
/// [EIP1014](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1014.md)
///
/// keccak256( 0xff ++ senderAddress ++ salt ++ keccak256(init_code))[12..]
///
/// where `salt` is always 32 bytes (a stack item).
pub fn get_create2_address(
    from: impl Into<Address>,
    salt: [u8; 32],
    init_code: impl AsRef<[u8]>,
) -> Address {
    let init_code_hash = keccak256(init_code);
    get_create2_address_from_hash(from, salt, init_code_hash)
}

/// Returns the CREATE2 address of a smart contract as specified in
/// [EIP1014](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1014.md),
/// taking the pre-computed hash of the init code as input.
///
/// keccak256( 0xff ++ senderAddress ++ salt ++ keccak256(init_code))[12..]
pub fn get_create2_address_from_hash(
    from: impl Into<Address>,
    salt: [u8; 32],
    init_code_hash: impl Into<H256>,
) -> Address {
    let from = from.into();
    let init_code_hash = init_code_hash.into();
    // always 85 bytes: 0xff+20+salt+code_hash
    let mut preimage = [0xff; 85];

    // 20bytes address
    preimage[1..21].copy_from_slice(from.as_bytes());
    // 32bytes salt
    preimage[21..53].copy_from_slice(&salt[..]);
    // 32bytes code hash
    preimage[53..].copy_from_slice(init_code_hash.as_ref());

    let hash = keccak256(&preimage[..]);
    let addr: [u8; 20] = hash[12..].try_into().expect("correct len");
    Address::from(addr)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let address = get_contract_address(from, U256::from(nonce));
            assert_eq!(address, expected.parse::<Address>().unwrap());
        }
    }

    #[test]
    // Test vectors from https://github.com/ethereum/EIPs/blob/master/EIPS/eip-1014.md#examples
    fn create2_address() {
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
            // get_create2_address()
            let from = from.parse::<Address>().unwrap();
            let salt = hex::decode(salt).unwrap();
            let init_code = hex::decode(init_code).unwrap();
            let expected = expected.parse::<Address>().unwrap();
            assert_eq!(expected, get_create2_address(from, salt.clone().try_into().unwrap(), init_code.clone()));

            // get_create2_address_from_hash()
            let init_code_hash = keccak256(init_code);
            assert_eq!(expected, get_create2_address_from_hash(from, salt.try_into().unwrap(), init_code_hash))
        }
    }
}
