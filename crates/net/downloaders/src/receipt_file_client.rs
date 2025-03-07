use std::{fmt, io};

use futures::Future;
use reth_primitives::Receipt;
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};
use tracing::{trace, warn};

use crate::{DecodedFileChunk, FileClientError};

/// Helper trait implemented for [`Decoder`] that decodes the receipt type.
pub trait ReceiptDecoder: Decoder<Item = Option<ReceiptWithBlockNumber<Self::Receipt>>> {
    /// The receipt type being decoded.
    type Receipt;
}

impl<T, R> ReceiptDecoder for T
where
    T: Decoder<Item = Option<ReceiptWithBlockNumber<R>>>,
{
    type Receipt = R;
}

/// File client for reading RLP encoded receipts from file. Receipts in file must be in sequential
/// order w.r.t. block number.
#[derive(Debug)]
pub struct ReceiptFileClient<D: ReceiptDecoder> {
    /// The buffered receipts, read from file, as nested lists. One list per block number.
    pub receipts: Vec<Vec<D::Receipt>>,
    /// First (lowest) block number read from file.
    pub first_block: u64,
    /// Total number of receipts. Count of elements in receipts flattened.
    pub total_receipts: usize,
}

/// Constructs a file client from a reader and decoder.
pub trait FromReceiptReader {
    /// Error returned by file client type.
    type Error: From<io::Error>;

    /// Returns a file client
    fn from_receipt_reader<B>(
        reader: B,
        num_bytes: u64,
        prev_chunk_highest_block: Option<u64>,
    ) -> impl Future<Output = Result<DecodedFileChunk<Self>, Self::Error>>
    where
        Self: Sized,
        B: AsyncReadExt + Unpin;
}

impl<D> FromReceiptReader for ReceiptFileClient<D>
where
    D: ReceiptDecoder<Error = FileClientError> + fmt::Debug + Default,
{
    type Error = D::Error;

    /// Initialize the [`ReceiptFileClient`] from bytes that have been read from file. Caution! If
    /// first block has no transactions, it's assumed to be the genesis block.
    fn from_receipt_reader<B>(
        reader: B,
        num_bytes: u64,
        prev_chunk_highest_block: Option<u64>,
    ) -> impl Future<Output = Result<DecodedFileChunk<Self>, Self::Error>>
    where
        B: AsyncReadExt + Unpin,
    {
        let mut receipts = Vec::default();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream = FramedRead::with_capacity(reader, D::default(), num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            codec=?D::default(),
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

                        remaining_bytes = bytes;

                        break;
                    }
                    Err(err) => return Err(err),
                };

                match receipt {
                    Some(ReceiptWithBlockNumber { receipt, number }) => {
                        if block_number > number {
                            warn!(target: "downloaders::file", previous_block_number = block_number, "skipping receipt from a lower block: {number}");
                            continue;
                        }

                        total_receipts += 1;

                        if first_block.is_none() {
                            first_block = Some(number);
                            block_number = number;
                        }

                        if block_number == number {
                            receipts_for_block.push(receipt);
                        } else {
                            receipts.push(receipts_for_block);

                            // next block
                            block_number = number;
                            receipts_for_block = vec![receipt];
                        }
                    }
                    None => {
                        match first_block {
                            Some(num) => {
                                // if there was a block number before this, push receipts for that
                                // block
                                receipts.push(receipts_for_block);
                                // block with no txns
                                block_number = num + receipts.len() as u64;
                            }
                            None => {
                                // this is the first block and it's empty
                                if let Some(highest_block) = prev_chunk_highest_block {
                                    // this is a chunked read and this is not the first chunk
                                    block_number = highest_block + 1;
                                } else {
                                    // this is not a chunked read or this is the first chunk. assume
                                    // it's the genesis block
                                    block_number = 0;
                                }
                                first_block = Some(block_number);
                            }
                        }

                        receipts_for_block = vec![];
                    }
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

            // we need to push the last receipts
            receipts.push(receipts_for_block);

            trace!(target: "downloaders::file",
                blocks = receipts.len(),
                total_receipts,
                "Initialized receipt file client"
            );

            Ok(DecodedFileChunk {
                file_client: Self {
                    receipts,
                    first_block: first_block.unwrap_or_default(),
                    total_receipts,
                },
                remaining_bytes,
                highest_block: Some(block_number),
            })
        }
    }
}

/// [`Receipt`] with block number.
#[derive(Debug, PartialEq, Eq)]
pub struct ReceiptWithBlockNumber<R = Receipt> {
    /// Receipt.
    pub receipt: R,
    /// Block number.
    pub number: u64,
}
