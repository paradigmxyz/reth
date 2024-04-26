//! Codec for reading raw block bodies from a file.

use crate::receipt_file_client;
use alloy_rlp::{Decodable, Encodable};
use reth_primitives::{
    bytes::{Buf, BytesMut},
    Receipt,
};
use tokio_util::codec::{Decoder, Encoder};

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
    type Error = receipt_file_client::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }

        let buf_slice = &mut src.as_ref();
        let body =
            Receipt::decode(buf_slice).map_err(|err| Self::Error::Rlp(err, src.to_vec()))?;
        src.advance(src.len() - buf_slice.len());

        Ok(Some(body))
    }
}

impl Encoder<Receipt> for ReceiptFileCodec {
    type Error = FileClientError;

    fn encode(&mut self, item: Receipt, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst);
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
struct ReceiptWithBlockNumber {
    /// Receipt type.
    pub tx_type: TxType,
    /// If transaction is executed successfully.
    ///
    /// This is the `statusCode`
    pub success: bool,
    /// Gas used
    pub cumulative_gas_used: u64,
    /// Log send from contracts.
    pub logs: Vec<Log>,
    /// Block number.
    pub block_number: u64,
}
