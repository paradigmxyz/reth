//! Codec for reading raw receipts from a file.

use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_primitives::{
    bytes::{Buf, BytesMut}, Address, Bloom, Bytes, Log, Receipt, TxType, B256
};
use tokio_util::codec::{Decoder, Encoder};

use crate::op_receipt_file_client::{self, ReceiptWithBlockNumber};

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
    type Error = op_receipt_file_client::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }

        let buf_slice = &mut src.as_ref();
        let receipt =
            HackReceipt::decode(buf_slice).map_err(|err| Self::Error::Rlp(err, src.to_vec()))?;
        src.advance(src.len() - buf_slice.len());

        Ok(Some(receipt.into()))
    }
}

impl Encoder<Receipt> for ReceiptFileCodec {
    type Error = op_receipt_file_client::Error;

    fn encode(&mut self, item: Receipt, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst);
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct HackReceipt {
    // Consensus fields: These fields are defined by the Yellow Paper
    pub tx_type: TxType,
    pub post_state: Bytes,
    pub status: u64,
    pub cumulative_gas_used: u64,
    pub bloom: Bloom,
    pub logs: Vec<Log>,
    pub tx_hash: B256,
    pub contract_address: Address,
    pub gas_used: u64,
    pub block_hash: B256,
    pub block_number: u64,
    pub transaction_index: u64,
    pub l1_gas_price: u64,
    pub l1_gas_used: u64,
    pub l1_fee: u64,
    pub fee_scalar: String,
}

impl From<HackReceipt> for ReceiptWithBlockNumber {
    fn from(exported_receipt: HackReceipt) -> Self {
        let mut receipt = Receipt::default();
        receipt.tx_type = exported_receipt.tx_type;
        receipt.success = exported_receipt.status != 0;
        receipt.cumulative_gas_used = exported_receipt.cumulative_gas_used;
        receipt.logs = exported_receipt.logs;

        Self { receipt, number: exported_receipt.block_number }
    }
}
