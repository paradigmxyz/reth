//! Codec for reading raw receipts from a file.

use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_primitives::{
    bytes::{Buf, BytesMut},
    revm_primitives::LogData,
    Address, Log, Receipt, TxType, B256,
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

#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
struct GethLog {
    // Consensus fields:
    // address of the contract that generated the event
    address: Address,
    // list of topics provided by the contract.
    topics: Vec<B256>,
    // supplied by the contract, usually ABI-encoded
    data: Vec<u8>,

    // Derived fields. These fields are filled in by the node
    // but not secured by consensus.
    // block in which the transaction was included
    block_number: u64,
    // hash of the transaction
    transaction_hash: B256,
    // index of the transaction in the block
    transaction_index: u32,
    // hash of the block in which the transaction was included
    block_hash: B256,
    // index of the log in the block
    log_index: u32,

    // The Removed field is true if this log was reverted due to a chain reorganisation.
    // You must pay attention to this field if you receive logs through a filter query.
    removed: bool,
}
/// See <https://github.com/testinprod-io/op-geth/pull/1>
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
struct HackReceipt {
    tx_type: u8,
    post_state: Vec<u8>,
    status: u64,
    cumulative_gas_used: u64,
    bloom: Vec<u8>,
    logs: Vec<GethLog>,
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

#[allow(clippy::field_reassign_with_default)]
impl TryFrom<HackReceipt> for ReceiptWithBlockNumber {
    type Error = &'static str;
    fn try_from(exported_receipt: HackReceipt) -> Result<Self, Self::Error> {
        let mut receipt = Receipt::default();
        receipt.tx_type = TxType::try_from(exported_receipt.tx_type.to_be_bytes()[0])?;
        receipt.success = exported_receipt.status != 0;
        receipt.cumulative_gas_used = exported_receipt.cumulative_gas_used;

        let mut logs = Vec::with_capacity(exported_receipt.logs.len());
        for GethLog { address, topics, data, .. } in exported_receipt.logs {
            logs.push(Log {
                address,
                data: LogData::new(topics, data.into()).ok_or("cannot convert to log data")?,
            })
        }

        receipt.logs = logs;

        Ok(Self { receipt, number: exported_receipt.block_number })
    }
}

#[cfg(test)]
mod test {
    use reth_primitives::hex;

    use super::*;

    #[test]
    fn encode_decode() {
        const HACK_RECEIPT: [u8; 786] = hex!("f9030ff9030c8080018303183db9010000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000400000000000100000000000000200000000002000000000000001000000000000000000004000000000000000000000000000040000400000100400000000000000100000000000000000000000000000020000000000000000000000000000000000000000000000001000000000000000000000100000000000000000000000000000000000000000000000000000000000000088000000080000000000010000000000000000000000000000800008000120000000000000000000000000000000002000f90197f89b948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff863a00109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac60271a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2da000000000000000000000000000000000000000000000000000000000618d8837f89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68ca000000000000000000000000000000000000000000000000000000000d0e3ebf0a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d80f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aff842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234fa000000000000000000000000000000000000000000000007edc6ca0bb6834800080a05e77a04531c7c107af1882d76cbff9486d0a9aa53701c30888509d4f5f2b003a9400000000000000000000000000000000000000008303183da0bee7192e575af30420cae0c7776304ac196077ee72b048970549e4f08e8754530180018212c2821c2383312e35");

        let mut encoded = BytesMut::from(&HACK_RECEIPT[..]);

        let mut codec = ReceiptFileCodec;

        let decoded_hack_receipt = codec.decode(&mut encoded).unwrap();
    }
}
