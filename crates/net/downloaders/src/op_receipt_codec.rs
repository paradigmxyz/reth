//! Codec for reading raw receipts from a file.

use alloy_rlp::{Decodable, Encodable, Rlp};
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
pub(crate) struct ReceiptFileCodec;

impl Decoder for ReceiptFileCodec {
    type Item = ReceiptWithBlockNumber;
    type Error = FileClientError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }

        let buf_slice = &mut src.as_ref();
        let receipt =
            HackReceipt::decode(buf_slice).map_err(|err| Self::Error::Rlp(err, src.to_vec()))?;
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

/// See <https://github.com/testinprod-io/op-geth/pull/1>
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackReceipt {
    tx_type: u8,
    post_state: Bytes,
    status: u64,
    cumulative_gas_used: u64,
    bloom: Bloom,
    /// <https://github.com/testinprod-io/op-geth/blob/29062eb0fac595eeeddd3a182a25326405c66e05/core/types/log.go#L67-L72>
    logs: Vec<Log>,
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
            logs,
            ..Default::default()
        };

        Ok(Self { receipt, number })
    }
}

impl Decodable for HackReceipt {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let buf = &mut &alloy_rlp::Header::decode_bytes(buf, true).unwrap()[..];

        let mut rlp = Rlp::new(buf)?;

        let tx_type = rlp.get_next::<u8>()?.ok_or(alloy_rlp::Error::Custom("missing 'tx type'"))?;

        let post_state =
            rlp.get_next::<Bytes>()?.ok_or(alloy_rlp::Error::Custom("missing 'post state'"))?;

        let status = rlp.get_next::<u64>()?.ok_or(alloy_rlp::Error::Custom("missing 'status'"))?;

        let cumulative_gas_used = rlp
            .get_next::<u64>()?
            .ok_or(alloy_rlp::Error::Custom("missing 'cumulative gas used'"))?;

        let bloom = rlp.get_next::<Bloom>()?.ok_or(alloy_rlp::Error::Custom("missing 'bloom'"))?;

        let logs = rlp.get_next::<Vec<Log>>()?.ok_or(alloy_rlp::Error::Custom("missing 'logs'"))?;

        let tx_hash =
            rlp.get_next::<B256>()?.ok_or(alloy_rlp::Error::Custom("missing 'tx hash'"))?;

        let contract_address = rlp
            .get_next::<Address>()?
            .ok_or(alloy_rlp::Error::Custom("missing 'contract address'"))?;

        let gas_used =
            rlp.get_next::<u64>()?.ok_or(alloy_rlp::Error::Custom("missing 'gas used'"))?;

        let block_hash =
            rlp.get_next::<B256>()?.ok_or(alloy_rlp::Error::Custom("missing 'block hash'"))?;

        let block_number =
            rlp.get_next::<u64>()?.ok_or(alloy_rlp::Error::Custom("missing 'block number'"))?;

        let transaction_index =
            rlp.get_next::<u32>()?.ok_or(alloy_rlp::Error::Custom("missing 'tx index'"))?;

        let l1_gas_price =
            rlp.get_next::<u64>()?.ok_or(alloy_rlp::Error::Custom("missing 'l1 gas price'"))?;

        let l1_gas_used =
            rlp.get_next::<u64>()?.ok_or(alloy_rlp::Error::Custom("missing 'l1 gas used'"))?;

        let l1_fee = rlp.get_next::<u64>()?.ok_or(alloy_rlp::Error::Custom("missing 'l1 fee'"))?;

        let fee_scalar =
            rlp.get_next::<String>()?.ok_or(alloy_rlp::Error::Custom("missing 'fee scalar'"))?;

        Ok(HackReceipt {
            tx_type,
            post_state,
            status,
            cumulative_gas_used,
            bloom,
            logs,
            tx_hash,
            contract_address,
            gas_used,
            block_hash,
            block_number,
            transaction_index,
            l1_gas_price,
            l1_gas_used,
            l1_fee,
            fee_scalar,
        })
    }
}

#[cfg(test)]
mod test {
    use reth_primitives::{alloy_primitives::LogData, hex};

    use super::*;

    // RLP encoded OP mainnet `HackReceipt` of tx in block 1
    const HACK_RECEIPT_ENCODED: [u8; 786] = hex!("f9030ff9030c8080018303183db9010000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000400000000000100000000000000200000000002000000000000001000000000000000000004000000000000000000000000000040000400000100400000000000000100000000000000000000000000000020000000000000000000000000000000000000000000000001000000000000000000000100000000000000000000000000000000000000000000000000000000000000088000000080000000000010000000000000000000000000000800008000120000000000000000000000000000000002000f90197f89b948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff863a00109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac60271a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2da000000000000000000000000000000000000000000000000000000000618d8837f89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68ca000000000000000000000000000000000000000000000000000000000d0e3ebf0a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d80f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234fa000000000000000000000000000000000000000000000007edc6ca0bb6834800080a05e77a04531c7c107af1882d76cbff9486d0a9aa53701c30888509d4f5f2b003a9400000000000000000000000000000000000000008303183da0bee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e8754530180018212c2821c2383312e35");

    // Can't construct whole `ReceiptWithBlockNumber` as a constant due to `Vec<Log>`

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
            bloom: Bloom::from(hex!("00000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000400000000000100000000000000200000000002000000000000001000000000000000000004000000000000000000000000000040000400000100400000000000000100000000000000000000000000000020000000000000000000000000000000000000000000000001000000000000000000000100000000000000000000000000000000000000000000000000000000000000088000000080000000000010000000000000000000000000000800008000120000000000000000000000000000000002000")), logs: vec![log_1(), log_2(), log_3()],
            tx_hash: B256::from(hex!("5e77a04531c7c107af1882d76cbff9486d0a9aa53701c30888509d4f5f2b003a")), contract_address: Address::from(hex!("0000000000000000000000000000000000000000")), gas_used: 202813,
            block_hash: B256::from(hex!("bee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e875453")), block_number: 1, transaction_index: 0,
            l1_gas_price: 1,
            l1_gas_used: 4802, 
            l1_fee: 7203, 
            fee_scalar: String::from("1.5") 
        };

        let decoded = HackReceipt::decode(&mut &HACK_RECEIPT_ENCODED[..]).unwrap();

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

        let mut encoded = BytesMut::from(&HACK_RECEIPT_ENCODED[..]);

        let mut codec = ReceiptFileCodec;

        // test

        let decoded_hack_receipt = codec.decode(&mut encoded).unwrap().unwrap();

        assert_eq!(decoded, decoded_hack_receipt);
    }
}
