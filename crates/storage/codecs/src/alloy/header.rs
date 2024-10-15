use crate::Compact;
use alloy_consensus::Header as AlloyHeader;
use alloy_primitives::{Address, BlockNumber, Bloom, Bytes, B256, U256};

/// Block header
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`alloy_consensus::Header`]
#[cfg_attr(test, derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
pub(crate) struct Header {
    parent_hash: B256,
    ommers_hash: B256,
    beneficiary: Address,
    state_root: B256,
    transactions_root: B256,
    receipts_root: B256,
    withdrawals_root: Option<B256>,
    logs_bloom: Bloom,
    difficulty: U256,
    number: BlockNumber,
    gas_limit: u64,
    gas_used: u64,
    timestamp: u64,
    mix_hash: B256,
    nonce: u64,
    base_fee_per_gas: Option<u64>,
    blob_gas_used: Option<u64>,
    excess_blob_gas: Option<u64>,
    parent_beacon_block_root: Option<B256>,
    extra_fields: Option<HeaderExt>,
    extra_data: Bytes,
}

/// [`Header`] extension struct.
///
/// All new fields should be added here in the form of a `Option<T>`, since `Option<HeaderExt>` is
/// used as a field of [`Header`] for backwards compatibility.
///
/// More information: <https://github.com/paradigmxyz/reth/issues/7820> & [`reth_codecs_derive::Compact`].
#[cfg_attr(test, derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
pub(crate) struct HeaderExt {
    requests_root: Option<B256>,
}

impl HeaderExt {
    /// Converts into [`Some`] if any of the field exists. Otherwise, returns [`None`].
    ///
    /// Required since [`Header`] uses `Option<HeaderExt>` as a field.
    const fn into_option(self) -> Option<Self> {
        if self.requests_root.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl Compact for AlloyHeader {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let extra_fields = HeaderExt { requests_root: self.requests_root };

        let header = Header {
            parent_hash: self.parent_hash,
            ommers_hash: self.ommers_hash,
            beneficiary: self.beneficiary,
            state_root: self.state_root,
            transactions_root: self.transactions_root,
            receipts_root: self.receipts_root,
            withdrawals_root: self.withdrawals_root,
            logs_bloom: self.logs_bloom,
            difficulty: self.difficulty,
            number: self.number,
            gas_limit: self.gas_limit,
            gas_used: self.gas_used,
            timestamp: self.timestamp,
            mix_hash: self.mix_hash,
            nonce: self.nonce.into(),
            base_fee_per_gas: self.base_fee_per_gas,
            blob_gas_used: self.blob_gas_used,
            excess_blob_gas: self.excess_blob_gas,
            parent_beacon_block_root: self.parent_beacon_block_root,
            extra_fields: extra_fields.into_option(),
            extra_data: self.extra_data.clone(),
        };
        header.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (header, _) = Header::from_compact(buf, len);
        let alloy_header = Self {
            parent_hash: header.parent_hash,
            ommers_hash: header.ommers_hash,
            beneficiary: header.beneficiary,
            state_root: header.state_root,
            transactions_root: header.transactions_root,
            receipts_root: header.receipts_root,
            withdrawals_root: header.withdrawals_root,
            logs_bloom: header.logs_bloom,
            difficulty: header.difficulty,
            number: header.number,
            gas_limit: header.gas_limit,
            gas_used: header.gas_used,
            timestamp: header.timestamp,
            mix_hash: header.mix_hash,
            nonce: header.nonce.into(),
            base_fee_per_gas: header.base_fee_per_gas,
            blob_gas_used: header.blob_gas_used,
            excess_blob_gas: header.excess_blob_gas,
            parent_beacon_block_root: header.parent_beacon_block_root,
            requests_root: header.extra_fields.and_then(|h| h.requests_root),
            extra_data: header.extra_data,
        };
        (alloy_header, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, bloom, bytes, hex};

    /// Holesky block #1947953
    const HOLESKY_BLOCK: Header = Header {
        parent_hash: b256!("8605e0c46689f66b3deed82598e43d5002b71a929023b665228728f0c6e62a95"),
        ommers_hash: b256!("1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"),
        beneficiary: address!("c6e2459991bfe27cca6d86722f35da23a1e4cb97"),
        state_root: b256!("edad188ca5647d62f4cca417c11a1afbadebce30d23260767f6f587e9b3b9993"),
        transactions_root: b256!("4daf25dc08a841aa22aa0d3cb3e1f159d4dcaf6a6063d4d36bfac11d3fdb63ee"),
        receipts_root: b256!("1a1500328e8ade2592bbea1e04f9a9fd8c0142d3175d6e8420984ee159abd0ed"),
        withdrawals_root: Some(b256!("d0f7f22d6d915be5a3b9c0fee353f14de5ac5c8ac1850b76ce9be70b69dfe37d")),
        logs_bloom: bloom!("36410880400480e1090a001c408880800019808000125124002100400048442220020000408040423088300004d0000050803000862485a02020011600a5010404143021800881e8e08c402940404002105004820c440051640000809c000011080002300208510808150101000038002500400040000230000000110442800000800204420100008110080200088c1610c0b80000c6008900000340400200200210010111020000200041a2010804801100030a0284a8463820120a0601480244521002a10201100400801101006002001000008000000ce011011041086418609002000128800008180141002003004c00800040940c00c1180ca002890040"),
        difficulty: U256::ZERO,
        number: 0x1db931,
        gas_limit: 0x1c9c380,
        gas_used: 0x440949,
        timestamp: 0x66982980,
        mix_hash: b256!("574db0ff0a2243b434ba2a35da8f2f72df08bca44f8733f4908d10dcaebc89f1"),
        nonce: 0,
        base_fee_per_gas: Some(0x8),
        blob_gas_used: Some(0x60000),
        excess_blob_gas: Some(0x0),
        parent_beacon_block_root: Some(b256!("aa1d9606b7932f2280a19b3498b9ae9eebc6a83f1afde8e45944f79d353db4c1")),
        extra_data: bytes!("726574682f76312e302e302f6c696e7578"),
        extra_fields: None,
    };

    #[test]
    fn test_ensure_backwards_compatibility() {
        assert_eq!(Header::bitflag_encoded_bytes(), 4);
        assert_eq!(HeaderExt::bitflag_encoded_bytes(), 1);
    }

    #[test]
    fn test_backwards_compatibility() {
        let holesky_header_bytes = hex!("81a121788605e0c46689f66b3deed82598e43d5002b71a929023b665228728f0c6e62a951dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347c6e2459991bfe27cca6d86722f35da23a1e4cb97edad188ca5647d62f4cca417c11a1afbadebce30d23260767f6f587e9b3b99934daf25dc08a841aa22aa0d3cb3e1f159d4dcaf6a6063d4d36bfac11d3fdb63ee1a1500328e8ade2592bbea1e04f9a9fd8c0142d3175d6e8420984ee159abd0edd0f7f22d6d915be5a3b9c0fee353f14de5ac5c8ac1850b76ce9be70b69dfe37d36410880400480e1090a001c408880800019808000125124002100400048442220020000408040423088300004d0000050803000862485a02020011600a5010404143021800881e8e08c402940404002105004820c440051640000809c000011080002300208510808150101000038002500400040000230000000110442800000800204420100008110080200088c1610c0b80000c6008900000340400200200210010111020000200041a2010804801100030a0284a8463820120a0601480244521002a10201100400801101006002001000008000000ce011011041086418609002000128800008180141002003004c00800040940c00c1180ca0028900401db93101c9c38044094966982980574db0ff0a2243b434ba2a35da8f2f72df08bca44f8733f4908d10dcaebc89f101080306000000aa1d9606b7932f2280a19b3498b9ae9eebc6a83f1afde8e45944f79d353db4c1726574682f76312e302e302f6c696e7578");
        let (decoded_header, _) =
            Header::from_compact(&holesky_header_bytes, holesky_header_bytes.len());

        assert_eq!(decoded_header, HOLESKY_BLOCK);

        let mut encoded_header = Vec::with_capacity(holesky_header_bytes.len());
        assert_eq!(holesky_header_bytes.len(), decoded_header.to_compact(&mut encoded_header));
        assert_eq!(encoded_header, holesky_header_bytes);
    }

    #[test]
    fn test_extra_fields() {
        let mut header = HOLESKY_BLOCK;
        header.extra_fields = Some(HeaderExt { requests_root: Some(B256::random()) });

        let mut encoded_header = vec![];
        let len = header.to_compact(&mut encoded_header);
        assert_eq!(header, Header::from_compact(&encoded_header, len).0);
    }

    #[test]
    fn test_extra_fields_missing() {
        let mut header = HOLESKY_BLOCK;
        header.extra_fields = None;

        let mut encoded_header = vec![];
        let len = header.to_compact(&mut encoded_header);
        assert_eq!(header, Header::from_compact(&encoded_header, len).0);
    }
}
