//! Header types.

use alloy_rlp::{Decodable, Encodable};
use bytes::BufMut;
use reth_codecs_derive::add_arbitrary_tests;

/// Represents the direction for a headers request depending on the `reverse` field of the request.
/// > The response must contain a number of block headers, of rising number when reverse is 0,
/// > falling when 1
///
/// Ref: <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03>
///
/// [`HeadersDirection::Rising`] block numbers for `reverse == 0 == false`
/// [`HeadersDirection::Falling`] block numbers for `reverse == 1 == true`
///
/// See also <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03>
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub enum HeadersDirection {
    /// Falling block number.
    Falling,
    /// Rising block number.
    #[default]
    Rising,
}

impl HeadersDirection {
    /// Returns true for rising block numbers
    pub const fn is_rising(&self) -> bool {
        matches!(self, Self::Rising)
    }

    /// Returns true for falling block numbers
    pub const fn is_falling(&self) -> bool {
        matches!(self, Self::Falling)
    }

    /// Converts the bool into a direction.
    ///
    /// Returns:
    ///
    /// [`HeadersDirection::Rising`] block numbers for `reverse == 0 == false`
    /// [`HeadersDirection::Falling`] block numbers for `reverse == 1 == true`
    pub const fn new(reverse: bool) -> Self {
        if reverse {
            Self::Falling
        } else {
            Self::Rising
        }
    }
}

impl Encodable for HeadersDirection {
    fn encode(&self, out: &mut dyn BufMut) {
        bool::from(*self).encode(out)
    }

    fn length(&self) -> usize {
        bool::from(*self).length()
    }
}

impl Decodable for HeadersDirection {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let value: bool = Decodable::decode(buf)?;
        Ok(value.into())
    }
}

impl From<bool> for HeadersDirection {
    fn from(reverse: bool) -> Self {
        Self::new(reverse)
    }
}

impl From<HeadersDirection> for bool {
    fn from(value: HeadersDirection) -> Self {
        match value {
            HeadersDirection::Rising => false,
            HeadersDirection::Falling => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, bloom, bytes, hex, Address, Bytes, B256, U256};
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives::Header;
    use std::str::FromStr;

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn test_encode_block_header() {
        let expected = hex!("f901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let header = Header {
            difficulty: U256::from(0x8ae_u64),
            number: 0xd05_u64,
            gas_limit: 0x115c,
            gas_used: 0x15b3,
            timestamp: 0x1a0a_u64,
            extra_data: Bytes::from_str("7788").unwrap(),
            ommers_hash: B256::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            ..Default::default()
        };
        let mut data = vec![];
        header.encode(&mut data);
        assert_eq!(hex::encode(&data), hex::encode(expected));
        assert_eq!(header.length(), data.len());
    }

    // Test vector from: https://github.com/ethereum/tests/blob/f47bbef4da376a49c8fc3166f09ab8a6d182f765/BlockchainTests/ValidBlocks/bcEIP1559/baseFee.json#L15-L36
    #[test]
    fn test_eip1559_block_header_hash() {
        let expected_hash =
            B256::from_str("6a251c7c3c5dca7b42407a3752ff48f3bbca1fab7f9868371d9918daf1988d1f")
                .unwrap();
        let header = Header {
            parent_hash: b256!("e0a94a7a3c9617401586b1a27025d2d9671332d22d540e0af72b069170380f2a"),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: address!("ba5e000000000000000000000000000000000000"),
            state_root: b256!("ec3c94b18b8a1cff7d60f8d258ec723312932928626b4c9355eb4ab3568ec7f7"),
            transactions_root: b256!("50f738580ed699f0469702c7ccc63ed2e51bc034be9479b7bff4e68dee84accf"),
            receipts_root: b256!("29b0562f7140574dd0d50dee8a271b22e1a0a7b78fca58f7c60370d8317ba2a9"),
            logs_bloom: bloom!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            difficulty: U256::from(0x020000),
            number: 0x01_u64,
            gas_limit: 0x016345785d8a0000,
            gas_used: 0x015534,
            timestamp: 0x079e,
            extra_data: bytes!("42"),
            mix_hash: b256!("0000000000000000000000000000000000000000000000000000000000000000"),
            nonce: 0u64.into()  ,
            base_fee_per_gas: Some(0x036b),
            withdrawals_root: None,
            blob_gas_used: None,
            excess_blob_gas: None,
            parent_beacon_block_root: None,
            requests_root: None
        };
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn test_decode_block_header() {
        let data = hex!("f901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let expected = Header {
            difficulty: U256::from(0x8aeu64),
            number: 0xd05u64,
            gas_limit: 0x115c,
            gas_used: 0x15b3,
            timestamp: 0x1a0au64,
            extra_data: Bytes::from_str("7788").unwrap(),
            ommers_hash: B256::ZERO,
            state_root: B256::ZERO,
            transactions_root: B256::ZERO,
            receipts_root: B256::ZERO,
            ..Default::default()
        };
        let header = <Header as Decodable>::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        // make sure the hash matches
        let expected_hash =
            B256::from_str("8c2f2af15b7b563b6ab1e09bed0e9caade7ed730aec98b70a993597a797579a9")
                .unwrap();
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://github.com/ethereum/tests/blob/970503935aeb76f59adfa3b3224aabf25e77b83d/BlockchainTests/ValidBlocks/bcExample/shanghaiExample.json#L15-L34
    #[test]
    fn test_decode_block_header_with_withdrawals() {
        let data = hex!("f9021ca018db39e19931515b30b16b3a92c292398039e31d6c267111529c3f2ba0a26c17a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa095efce3d6972874ca8b531b233b7a1d1ff0a56f08b20c8f1b89bef1b001194a5a071e515dd89e8a7973402c2e11646081b4e2209b2d3a1550df5095289dabcb3fba0ed9c51ea52c968e552e370a77a41dac98606e98b915092fb5f949d6452fce1c4b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff830125b882079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a027f166f1d7c789251299535cb176ba34116e44894476a7886fe5d73d9be5c973");
        let expected = Header {
            parent_hash: B256::from_str(
                "18db39e19931515b30b16b3a92c292398039e31d6c267111529c3f2ba0a26c17",
            )
            .unwrap(),
            beneficiary: Address::from_str("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba").unwrap(),
            state_root: B256::from_str(
                "95efce3d6972874ca8b531b233b7a1d1ff0a56f08b20c8f1b89bef1b001194a5",
            )
            .unwrap(),
            transactions_root: B256::from_str(
                "71e515dd89e8a7973402c2e11646081b4e2209b2d3a1550df5095289dabcb3fb",
            )
            .unwrap(),
            receipts_root: B256::from_str(
                "ed9c51ea52c968e552e370a77a41dac98606e98b915092fb5f949d6452fce1c4",
            )
            .unwrap(),
            number: 0x01,
            gas_limit: 0x7fffffffffffffff,
            gas_used: 0x0125b8,
            timestamp: 0x079e,
            extra_data: Bytes::from_str("42").unwrap(),
            mix_hash: B256::from_str(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            )
            .unwrap(),
            base_fee_per_gas: Some(0x09),
            withdrawals_root: Some(
                B256::from_str("27f166f1d7c789251299535cb176ba34116e44894476a7886fe5d73d9be5c973")
                    .unwrap(),
            ),
            ..Default::default()
        };
        let header = <Header as Decodable>::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        let expected_hash =
            B256::from_str("85fdec94c534fa0a1534720f167b899d1fc268925c71c0cbf5aaa213483f5a69")
                .unwrap();
        assert_eq!(header.hash_slow(), expected_hash);
    }

    // Test vector from: https://github.com/ethereum/tests/blob/7e9e0940c0fcdbead8af3078ede70f969109bd85/BlockchainTests/ValidBlocks/bcExample/cancunExample.json
    #[test]
    fn test_decode_block_header_with_blob_fields_ef_tests() {
        let data = hex!("f90221a03a9b485972e7353edd9152712492f0c58d89ef80623686b6bf947a4a6dce6cb6a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa03c837fc158e3e93eafcaf2e658a02f5d8f99abc9f1c4c66cdea96c0ca26406aea04409cc4b699384ba5f8248d92b784713610c5ff9c1de51e9239da0dac76de9cea046cab26abf1047b5b119ecc2dda1296b071766c8b1307e1381fcecc90d513d86b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8302a86582079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b4218302000080");
        let expected = Header {
            parent_hash: B256::from_str(
                "3a9b485972e7353edd9152712492f0c58d89ef80623686b6bf947a4a6dce6cb6",
            )
            .unwrap(),
            ommers_hash: B256::from_str(
                "1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
            )
            .unwrap(),
            beneficiary: Address::from_str("2adc25665018aa1fe0e6bc666dac8fc2697ff9ba").unwrap(),
            state_root: B256::from_str(
                "3c837fc158e3e93eafcaf2e658a02f5d8f99abc9f1c4c66cdea96c0ca26406ae",
            )
            .unwrap(),
            transactions_root: B256::from_str(
                "4409cc4b699384ba5f8248d92b784713610c5ff9c1de51e9239da0dac76de9ce",
            )
            .unwrap(),
            receipts_root: B256::from_str(
                "46cab26abf1047b5b119ecc2dda1296b071766c8b1307e1381fcecc90d513d86",
            )
            .unwrap(),
            logs_bloom: Default::default(),
            difficulty: U256::from(0),
            number: 0x1,
            gas_limit: 0x7fffffffffffffff,
            gas_used: 0x02a865,
            timestamp: 0x079e,
            extra_data: Bytes::from(vec![0x42]),
            mix_hash: B256::from_str(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421",
            )
            .unwrap(),
            nonce: 0u64.into(),
            base_fee_per_gas: Some(9),
            withdrawals_root: Some(
                B256::from_str("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421")
                    .unwrap(),
            ),
            blob_gas_used: Some(0x020000),
            excess_blob_gas: Some(0),
            parent_beacon_block_root: None,
            requests_root: None,
        };

        let header = Header::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        let expected_hash =
            B256::from_str("0x10aca3ebb4cf6ddd9e945a5db19385f9c105ede7374380c50d56384c3d233785")
                .unwrap();
        assert_eq!(header.hash_slow(), expected_hash);
    }

    #[test]
    fn test_decode_block_header_with_blob_fields() {
        // Block from devnet-7
        let data = hex!("f90239a013a7ec98912f917b3e804654e37c9866092043c13eb8eab94eb64818e886cff5a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d4934794f97e180c050e5ab072211ad2c213eb5aee4df134a0ec229dbe85b0d3643ad0f471e6ec1a36bbc87deffbbd970762d22a53b35d068aa056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000080830305988401c9c380808464c40d5499d883010c01846765746888676f312e32302e35856c696e7578a070ccadc40b16e2094954b1064749cc6fbac783c1712f1b271a8aac3eda2f232588000000000000000007a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421808401600000");
        let expected = Header {
            parent_hash: B256::from_str(
                "13a7ec98912f917b3e804654e37c9866092043c13eb8eab94eb64818e886cff5",
            )
            .unwrap(),
            ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
            beneficiary: address!("f97e180c050e5ab072211ad2c213eb5aee4df134"),
            state_root: b256!("ec229dbe85b0d3643ad0f471e6ec1a36bbc87deffbbd970762d22a53b35d068a"),
            transactions_root: b256!(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
            ),
            receipts_root: b256!(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
            ),
            logs_bloom: Default::default(),
            difficulty: U256::from(0),
            number: 0x30598,
            gas_limit: 0x1c9c380,
            gas_used: 0,
            timestamp: 0x64c40d54,
            extra_data: bytes!("d883010c01846765746888676f312e32302e35856c696e7578"),
            mix_hash: b256!("70ccadc40b16e2094954b1064749cc6fbac783c1712f1b271a8aac3eda2f2325"),
            nonce: 0u64.into(),
            base_fee_per_gas: Some(7),
            withdrawals_root: Some(b256!(
                "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
            )),
            parent_beacon_block_root: None,
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0x1600000),
            requests_root: None,
        };

        let header = Header::decode(&mut data.as_slice()).unwrap();
        assert_eq!(header, expected);

        let expected_hash =
            b256!("539c9ea0a3ca49808799d3964b8b6607037227de26bc51073c6926963127087b");
        assert_eq!(header.hash_slow(), expected_hash);
    }

    #[test]
    fn sanity_direction() {
        let reverse = true;
        assert_eq!(HeadersDirection::Falling, reverse.into());
        assert_eq!(reverse, bool::from(HeadersDirection::Falling));

        let reverse = false;
        assert_eq!(HeadersDirection::Rising, reverse.into());
        assert_eq!(reverse, bool::from(HeadersDirection::Rising));

        let mut buf = Vec::new();
        let direction = HeadersDirection::Falling;
        direction.encode(&mut buf);
        assert_eq!(direction, HeadersDirection::decode(&mut buf.as_slice()).unwrap());

        let mut buf = Vec::new();
        let direction = HeadersDirection::Rising;
        direction.encode(&mut buf);
        assert_eq!(direction, HeadersDirection::decode(&mut buf.as_slice()).unwrap());
    }
}
