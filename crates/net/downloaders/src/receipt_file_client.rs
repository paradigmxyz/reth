use futures::Future;
use reth_primitives::{Receipt, Receipts};
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;
use tracing::trace;

use crate::{
    file_client::{FileClientError, FromReader},
    file_codec_ovm_receipt::HackReceiptFileCodec,
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

    /// Initialize the [`ReceiptFileClient`] from bytes that have been read from file. Caution! If
    /// first block has no transactions, it's assumed to be the genesis block.
    fn from_reader<B>(
        reader: B,
        num_bytes: u64,
    ) -> impl Future<Output = Result<(Self, Vec<u8>), Self::Error>>
    where
        B: AsyncReadExt + Unpin,
    {
        let mut receipts = Receipts::new();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream =
            FramedRead::with_capacity(reader, HackReceiptFileCodec, num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            coded=?HackReceiptFileCodec,
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

                        break
                    }
                    Err(err) => return Err(err),
                };

                total_receipts += 1;

                match receipt {
                    Some(ReceiptWithBlockNumber { receipt, number }) => {
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
                                // this is the first block and it's empty, assume it's the genesis
                                // block
                                first_block = Some(0);
                                block_number = 0;
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

            Ok((
                Self { receipts, first_block: first_block.unwrap_or_default(), total_receipts },
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

#[cfg(test)]
mod test {
    use reth_primitives::hex;
    use reth_tracing::init_test_tracing;

    use crate::file_codec_ovm_receipt::test::{
        receipt_block_1 as op_mainnet_receipt_block_1,
        receipt_block_2 as op_mainnet_receipt_block_2,
        receipt_block_3 as op_mainnet_receipt_block_3,
        HACK_RECEIPT_ENCODED_BLOCK_1 as HACK_RECEIPT_ENCODED_BLOCK_1_OP_MAINNET,
        HACK_RECEIPT_ENCODED_BLOCK_2 as HACK_RECEIPT_ENCODED_BLOCK_2_OP_MAINNET,
        HACK_RECEIPT_ENCODED_BLOCK_3 as HACK_RECEIPT_ENCODED_BLOCK_3_OP_MAINNET,
    };

    use super::*;

    /// No receipts for genesis block
    const HACK_RECEIPT_BLOCK_NO_TRANSACTIONS: &[u8] = &hex!("c0");

    #[tokio::test]
    async fn receipt_file_client_ovm_codec() {
        init_test_tracing();

        // genesis block has no hack receipts
        let mut encoded_receipts = HACK_RECEIPT_BLOCK_NO_TRANSACTIONS.to_vec();
        // one receipt each for block 1 and 2
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_1_OP_MAINNET);
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_2_OP_MAINNET);
        // no receipt for block 4
        encoded_receipts.extend_from_slice(HACK_RECEIPT_BLOCK_NO_TRANSACTIONS);

        let encoded_byte_len = encoded_receipts.len() as u64;
        let reader = &mut &encoded_receipts[..];

        let (ReceiptFileClient { receipts, first_block, total_receipts }, _remaining_bytes) =
            ReceiptFileClient::from_reader(reader, encoded_byte_len).await.unwrap();

        assert_eq!(4, total_receipts);
        assert_eq!(0, first_block);
        assert!(receipts[0].is_empty());
        assert_eq!(op_mainnet_receipt_block_1().receipt, receipts[1][0].clone().unwrap());
        assert_eq!(op_mainnet_receipt_block_2().receipt, receipts[2][0].clone().unwrap());
        assert!(receipts[3].is_empty());
    }

    #[tokio::test]
    async fn no_receipts_middle_block() {
        init_test_tracing();

        // genesis block has no hack receipts
        let mut encoded_receipts = HACK_RECEIPT_BLOCK_NO_TRANSACTIONS.to_vec();
        // one receipt each for block 1
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_1_OP_MAINNET);
        // no receipt for block 2
        encoded_receipts.extend_from_slice(HACK_RECEIPT_BLOCK_NO_TRANSACTIONS);
        // one receipt for block 3
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_3_OP_MAINNET);

        let encoded_byte_len = encoded_receipts.len() as u64;
        let reader = &mut &encoded_receipts[..];

        let (ReceiptFileClient { receipts, first_block, total_receipts }, _remaining_bytes) =
            ReceiptFileClient::from_reader(reader, encoded_byte_len).await.unwrap();

        assert_eq!(4, total_receipts);
        assert_eq!(0, first_block);
        assert!(receipts[0].is_empty());
        assert_eq!(op_mainnet_receipt_block_1().receipt, receipts[1][0].clone().unwrap());
        assert!(receipts[2].is_empty());
        assert_eq!(op_mainnet_receipt_block_3().receipt, receipts[3][0].clone().unwrap());
    }

    #[tokio::test]
    async fn two_receipts_same_block() {
        init_test_tracing();

        // genesis block has no hack receipts
        let mut encoded_receipts = HACK_RECEIPT_BLOCK_NO_TRANSACTIONS.to_vec();
        // one receipt each for block 1
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_1_OP_MAINNET);
        // two receipts for block 2
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_2_OP_MAINNET);
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_2_OP_MAINNET);
        // one receipt for block 3
        encoded_receipts.extend_from_slice(HACK_RECEIPT_ENCODED_BLOCK_3_OP_MAINNET);

        let encoded_byte_len = encoded_receipts.len() as u64;
        let reader = &mut &encoded_receipts[..];

        let (ReceiptFileClient { receipts, first_block, total_receipts }, _remaining_bytes) =
            ReceiptFileClient::from_reader(reader, encoded_byte_len).await.unwrap();

        assert_eq!(5, total_receipts);
        assert_eq!(0, first_block);
        assert!(receipts[0].is_empty());
        assert_eq!(op_mainnet_receipt_block_1().receipt, receipts[1][0].clone().unwrap());
        assert_eq!(op_mainnet_receipt_block_2().receipt, receipts[2][0].clone().unwrap());
        assert_eq!(op_mainnet_receipt_block_2().receipt, receipts[2][1].clone().unwrap());
        assert_eq!(op_mainnet_receipt_block_3().receipt, receipts[3][0].clone().unwrap());
    }
}
