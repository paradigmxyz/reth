use alloy_rlp::EMPTY_LIST_CODE;
use futures::Future;
use reth_primitives::{Receipt, Receipts};
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use tracing::trace;

use crate::{
    file_client::{FileClientError, FromReader},
    file_codec_ovm_receipt::ReceiptFileCodec,
};

/// File client for reading RLP encoded receipts from file. Receipts in file must be in sequential
/// order w.r.t. block number.
#[derive(Debug)]
pub struct ReceiptFileClient {
    /// The buffered receipts, read from file, as nested lists. One list per block number.
    pub receipts: Receipts,
    /// First (lowest) block number read from file.
    pub first_block: u64,
    /// Total number of receipts. Count of elements in [`Receipts`] flattened.
    pub total_receipts: usize,
}

impl FromReader for ReceiptFileClient {
    type Error = FileClientError;

    /// Initialize the [`ReceiptFileClient`] from bytes that have been read from file.
    fn from_reader<B>(
        reader: B,
        num_bytes: u64,
    ) -> impl Future<Output = Result<(Self, Vec<u8>), Self::Error>>
    where
        B: AsyncReadExt + Unpin,
    {
        let mut receipts = Receipts::new();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream = FramedRead::with_capacity(reader, ReceiptFileCodec, num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            coded=?ReceiptFileCodec,
            "init decode stream"
        );

        let mut remaining_bytes = vec![];

        let mut log_interval = 0;
        let mut log_interval_start_block = 0;

        let mut block_number = 0;
        let mut total_receipts = 0;
        let mut receipts_for_block = vec![];
        let mut first_block = None;

        async move {
            while let Some(receipt_res) = stream.next().await {
                let receipt = match receipt_res {
                    Ok(receipt) => receipt,
                    Err(FileClientError::Rlp(err, bytes)) => {
                        trace!(target: "downloaders::file",
                            %err,
                            bytes_len=bytes.len(),
                            "partial receipt returned from decoding chunk"
                        );

                        // Empty list code indicates no receipts for the block, and no other receipt
                        // info
                        if !bytes.is_empty() && bytes[0] == EMPTY_LIST_CODE {
                            // empty list, so no receipts, just advance
                            remaining_bytes = bytes[1..].into();
                        } else {
                            remaining_bytes = bytes;
                        }

                        break
                    }
                    Err(err) => return Err(err),
                };

                total_receipts += 1;

                let ReceiptWithBlockNumber { receipt, number } = receipt;

                if first_block.is_none() {
                    first_block = Some(number);
                    block_number = number;
                }

                if block_number == number {
                    receipts_for_block.push(Some(receipt));
                } else {
                    receipts.push(receipts_for_block);

                    // next block
                    block_number = number;
                    receipts_for_block = vec![Some(receipt)];
                }

                if log_interval == 0 {
                    trace!(target: "downloaders::file",
                        block_number,
                        total_receipts,
                        "read first receipt"
                    );
                    log_interval_start_block = block_number;
                } else if log_interval % 100_000 == 0 {
                    trace!(target: "downloaders::file",
                        blocks=?log_interval_start_block..=block_number,
                        total_receipts,
                        "read receipts from file"
                    );
                    log_interval_start_block = block_number + 1;
                }
                log_interval += 1;
            }

            trace!(target: "downloaders::file",
                blocks=?log_interval_start_block..=block_number,
                total_receipts,
                "read receipts from file"
            );

            trace!(target: "downloaders::file",
                blocks = receipts.len(),
                total_receipts,
                ?receipts_for_block,
                "Initialized receipt file client"
            );

            // we need to push the last receipts, if any
            receipts.push(receipts_for_block);

            Ok((
                Self { receipts, first_block: first_block.unwrap(), total_receipts },
                remaining_bytes,
            ))
        }
    }
}

/// [`Receipt`] with block number.
#[derive(Debug, PartialEq, Eq)]
pub struct ReceiptWithBlockNumber {
    /// Receipt.
    pub receipt: Receipt,
    /// Block number.
    pub number: u64,
}
