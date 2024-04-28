//! Codec for reading raw receipts from a file.

use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpDecodableWrapper};
use reth_primitives::{
    bytes::{Buf, BytesMut},
    Address, Bloom, Bytes, Log, Receipt, TxType, B256,
};
use tokio_util::codec::{Decoder, Encoder};

use crate::{file_client::FileClientError, receipt_file_client::ReceiptWithBlockNumber};

/// Codec for reading raw receipts from a file.
///
/// If using with [`FramedRead`](tokio_util::codec::FramedRead), the user should make sure the
/// framed reader has capacity for the entire receipts file. Otherwise, the decoder will return
/// [`InputTooShort`](alloy_rlp::Error::InputTooShort), because RLP receipts can only be
/// decoded if the internal buffer is large enough to contain the entire receipt.
///
/// Without ensuring the framed reader has capacity for the entire file, a receipt is likely to
/// fall across two read buffers, the decoder will not be able to decode the receipt, which will
/// cause it to fail.
///
/// It's recommended to use [`with_capacity`](tokio_util::codec::FramedRead::with_capacity) to set
/// the capacity of the framed reader to the size of the file.
#[derive(Debug)]
pub struct ReceiptFileCodec;

impl Decoder for ReceiptFileCodec {
    type Item = ReceiptWithBlockNumber;
    type Error = FileClientError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }

        let buf_slice = &mut src.as_ref();
        let receipt = HackReceiptContainer::decode(buf_slice)
            .map_err(|err| Self::Error::Rlp(err, src.to_vec()))?
            .0;
        src.advance(src.len() - buf_slice.len());

        Ok(Some(receipt.try_into().map_err(FileClientError::from)?))
    }
}

impl Encoder<Receipt> for ReceiptFileCodec {
    type Error = FileClientError;

    fn encode(&mut self, item: Receipt, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst);
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, RlpDecodableWrapper)]
struct Logs(Vec<Log>);

/// See <https://github.com/testinprod-io/op-geth/pull/1>
#[derive(Debug, PartialEq, Eq, RlpDecodable)]
pub struct HackReceipt {
    tx_type: u8,
    post_state: Bytes,
    status: u64,
    cumulative_gas_used: u64,
    bloom: Bloom,
    /// <https://github.com/testinprod-io/op-geth/blob/29062eb0fac595eeeddd3a182a25326405c66e05/core/types/log.go#L67-L72>
    logs: Logs,
    tx_hash: B256,
    contract_address: Address,
    gas_used: u64,
    block_hash: B256,
    block_number: u64,
    transaction_index: u32,
    l1_gas_price: u64,
    l1_gas_used: u64,
    l1_fee: u64,
    fee_scalar: String,
}

#[derive(Debug, PartialEq, Eq, RlpDecodable)]
struct HackReceiptContainer(HackReceipt);

#[allow(clippy::needless_update)]
impl TryFrom<HackReceipt> for ReceiptWithBlockNumber {
    type Error = &'static str;
    fn try_from(exported_receipt: HackReceipt) -> Result<Self, Self::Error> {
        let HackReceipt {
            tx_type, status, cumulative_gas_used, logs, block_number: number, ..
        } = exported_receipt;

        let receipt = Receipt {
            tx_type: TxType::try_from(tx_type.to_be_bytes()[0])?,
            success: status != 0,
            cumulative_gas_used,
            logs: logs.0,
            ..Default::default()
        };

        Ok(Self { receipt, number })
    }
}

#[cfg(test)]
mod test {
    use reth_primitives::{alloy_primitives::LogData, hex};

    use super::*;

    const HACK_RECEIPT_ENCODED_1_AND_2: &[u8] = &hex!("f9030ff9030c8080018303183db9010000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000400000000000100000000000000200000000002000000000000001000000000000000000004000000000000000000000000000040000400000100400000000000000100000000000000000000000000000020000000000000000000000000000000000000000000000001000000000000000000000100000000000000000000000000000000000000000000000000000000000000088000000080000000000010000000000000000000000000000800008000120000000000000000000000000000000002000f90197f89b948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff863a00109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac60271a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2da000000000000000000000000000000000000000000000000000000000618d8837f89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68ca000000000000000000000000000000000000000000000000000000000d0e3ebf0a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d80f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234fa000000000000000000000000000000000000000000000007edc6ca0bb6834800080a05e77a04531c7c107af1882d76cbff9486d0a9aa53701c30888509d4f5f2b003a9400000000000000000000000000000000000000008303183da0bee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e8754530180018212c2821c2383312e35f90271f9026e8080018301c60db9010000080000000200000000000000000008000000000000000000000100008000000000000000000000000000000000000000000000000000000000400000000000100000000000000000000000020000000000000000000000000000000000004000000000000000000000000000000000400000000400000000000000100000000000000000000000000000020000000000000000000000000000000000000000100000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000008400000000000000000010000000000000000020000000020000000000000000000000000000000000000000000002000f8faf89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68ca000000000000000000000000000000000000000000000000000000000d0ea0e40a00000000000000000000000000000000000000000000000000000000000014218a0000000000000000000000000e5e7492282fd1e3bfac337a0beccd29b15b7b24080f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234fa000000000000000000000000000000000000000000000007eda7867e0c7d4800080a0af6ed8a6864d44989adc47c84f6fe0aeb1819817505c42cde6cbbcd5e14dd3179400000000000000000000000000000000000000008301c60da045fd6ce41bb8ebb2bccdaa92dd1619e287704cb07722039901a7eba63dea1d130280018212c2821c2383312e35");

    // RLP encoded OP mainnet `HackReceipt` of tx in block 1
    const HACK_RECEIPT_ENCODED: [u8; 786] = hex!("f9030ff9030c8080018303183db9010000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000400000000000100000000000000200000000002000000000000001000000000000000000004000000000000000000000000000040000400000100400000000000000100000000000000000000000000000020000000000000000000000000000000000000000000000001000000000000000000000100000000000000000000000000000000000000000000000000000000000000088000000080000000000010000000000000000000000000000800008000120000000000000000000000000000000002000f90197f89b948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff863a00109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac60271a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2da000000000000000000000000000000000000000000000000000000000618d8837f89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68ca000000000000000000000000000000000000000000000000000000000d0e3ebf0a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d80f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234fa000000000000000000000000000000000000000000000007edc6ca0bb6834800080a05e77a04531c7c107af1882d76cbff9486d0a9aa53701c30888509d4f5f2b003a9400000000000000000000000000000000000000008303183da0bee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e8754530180018212c2821c2383312e35");

    // Can't construct whole `ReceiptWithBlockNumber` as a constant due to `Logs`

    const TX_TYPE: TxType = TxType::Legacy;

    const SUCCESS: bool = true;

    const CUMULATIVE_GAS_USED: u64 = 202813;

    const BLOCK_NUMBER: u64 = 1;

    fn log_1() -> Log {
        Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850af")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "0109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac60271"
                    )),
                    B256::from(hex!(
                        "0000000000000000000000000000000000000000000000000000000000014218"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d"
                    )),
                ],
                Bytes::from(hex!(
                    "00000000000000000000000000000000000000000000000000000000618d8837"
                )),
            )
            .unwrap(),
        }
    }

    fn log_2() -> Log {
        Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850af")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "92e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68c"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000000000000000000000000000000000000d0e3ebf0"
                    )),
                    B256::from(hex!(
                        "0000000000000000000000000000000000000000000000000000000000014218"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d"
                    )),
                ],
                Bytes::default(),
            )
            .unwrap(),
        }
    }

    fn log_3() -> Log {
        Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850af")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234f"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000000000000000000000000007edc6ca0bb68348000"
                    )),
                ],
                Bytes::default(),
            )
            .unwrap(),
        }
    }

    #[test]
    fn decode_hack_receipt() {
        let receipt = HackReceipt {
            tx_type: TX_TYPE as u8,
            post_state: Bytes::default(),
            status: SUCCESS as u64,
            cumulative_gas_used: CUMULATIVE_GAS_USED,
            bloom: Bloom::from(hex!("00000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000400000000000100000000000000200000000002000000000000001000000000000000000004000000000000000000000000000040000400000100400000000000000100000000000000000000000000000020000000000000000000000000000000000000000000000001000000000000000000000100000000000000000000000000000000000000000000000000000000000000088000000080000000000010000000000000000000000000000800008000120000000000000000000000000000000002000")),
            logs: Logs(vec![log_1(), log_2(), log_3()]),
            tx_hash: B256::from(hex!("5e77a04531c7c107af1882d76cbff9486d0a9aa53701c30888509d4f5f2b003a")), contract_address: Address::from(hex!("0000000000000000000000000000000000000000")), gas_used: 202813,
            block_hash: B256::from(hex!("bee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e875453")),
            block_number: BLOCK_NUMBER,
            transaction_index: 0,
            l1_gas_price: 1,
            l1_gas_used: 4802,
            l1_fee: 7203,
            fee_scalar: String::from("1.5") 
        };

        let decoded = HackReceiptContainer::decode(&mut &HACK_RECEIPT_ENCODED[..]).unwrap().0;

        assert_eq!(receipt, decoded);
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn receipts_codec() {
        // rig

        // OP mainnet receipt of tx in block 1
        let decoded = ReceiptWithBlockNumber {
            receipt: Receipt {
                tx_type: TX_TYPE,
                success: SUCCESS,
                cumulative_gas_used: CUMULATIVE_GAS_USED,
                logs: vec![log_1(), log_2(), log_3()],
                ..Default::default()
            },
            number: BLOCK_NUMBER,
        };

        let encoded = &mut BytesMut::from(&HACK_RECEIPT_ENCODED_1_AND_2[..]);

        let mut codec = ReceiptFileCodec;

        // test

        let first = codec.decode(encoded).unwrap().unwrap();

        assert_eq!(decoded, first);

        let second = codec.decode(encoded).unwrap().unwrap();

        assert_eq!("ReceiptWithBlockNumber { receipt: Receipt { tx_type: Legacy, success: true, cumulative_gas_used: 116237, logs: [Log { address: 0x8ce8c13d816fe6daf12d6fd9e4952e1fc88850af, data: LogData { topics: [0x92e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68c, 0x00000000000000000000000000000000000000000000000000000000d0ea0e40, 0x0000000000000000000000000000000000000000000000000000000000014218, 0x000000000000000000000000e5e7492282fd1e3bfac337a0beccd29b15b7b240], data: 0x } }, Log { address: 0x8ce8c13d816fe6daf12d6fd9e4952e1fc88850af, data: LogData { topics: [0xfe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234f, 0x00000000000000000000000000000000000000000000007eda7867e0c7d48000], data: 0x } }] }, number: 2 }", format!("{:?}", second));
    }
}
