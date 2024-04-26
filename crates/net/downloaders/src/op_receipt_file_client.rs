use alloy_rlp::{Bytes, RlpDecodable, RlpEncodable};
use itertools::Either;
use reth_interfaces::p2p::{
    bodies::client::{BodiesClient, BodiesFut},
    download::DownloadClient,
    error::RequestError,
    headers::client::{HeadersClient, HeadersFut, HeadersRequest},
    priority::Priority,
};
use reth_primitives::{
    Address, BytesMut, Header, HeadersDirection, PeerId, Receipt, Receipts, SealedHeader, TxType, B256, U64
};
use std::{collections::HashMap, path::Path};
use thiserror::Error;
use tokio::{fs::File, io::AsyncReadExt};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use tracing::{debug, trace, warn};

use crate::op_receipt_codec::ReceiptFileCodec;

#[derive(Debug)]
pub struct ReceiptFileClient {
    /// The buffered receipts, read from file.
    receipts: Receipts,
}

/// An error that can occur when constructing and using a [`ReceiptFileClient`].
#[derive(Debug, Error)]
pub enum Error {
    /// An error occurred when opening or reading the file.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// An error occurred when decoding receipts from the file.
    #[error("{0}")]
    Rlp(alloy_rlp::Error, Vec<u8>),
}

impl ReceiptFileClient {
    /// Initialize the [`ReceiptFileClient`] from bytes that have been read from file.
    pub(crate) async fn from_reader<B>(reader: B, num_bytes: u64) -> Result<(Self, Vec<u8>), Error>
    where
        B: AsyncReadExt + Unpin,
    {
        let mut receipts = Receipts::new();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream = FramedRead::with_capacity(reader, ReceiptFileCodec, num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            "init decode stream"
        );

        let mut remaining_bytes = vec![];

        let mut log_interval = 0;
        let mut log_interval_start_block = 0;

        let mut block_number = 0;
        let mut receipts_for_block = vec![];

        while let Some(receipt_res) = stream.next().await {
            let receipt = match receipt_res {
                Ok(receipt) => receipt,
                Err(Error::Rlp(err, bytes)) => {
                    trace!(target: "downloaders::file",
                        %err,
                        bytes_len=bytes.len(),
                        "partial receipt returned from decoding chunk"
                    );
                    remaining_bytes = bytes;
                    break
                }
                Err(err) => return Err(err),
            };
            let ReceiptWithBlockNumber { receipt, number } = receipt;

            if block_number == number {
                receipts_for_block.push(Some(receipt));
            } else {
                // next block
                receipts.push(receipts_for_block);
                block_number = number;
                receipts_for_block = vec![Some(receipt)];
            }

            if log_interval == 0 {
                trace!(target: "downloaders::file",
                    block_number,
                    "read first receipt"
                );
                log_interval_start_block = block_number;
            } else if log_interval % 100_000 == 0 {
                trace!(target: "downloaders::file",
                    blocks=?log_interval_start_block..=block_number,
                    "read receipts from file"
                );
                log_interval_start_block = block_number + 1;
            }
            log_interval += 1;
        }

        trace!(target: "downloaders::file", receipts = receipts.len(), "Initialized file client");

        Ok((Self { receipts }, remaining_bytes))
    }
}

/// [`Receipt`] with block number.
#[derive(Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct ReceiptWithBlockNumber {
    /// Receipt.
    pub receipt: Receipt,
    /// Block number.
    pub number: u64,
}