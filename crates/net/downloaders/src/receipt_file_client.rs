use std::{fmt, io, marker::PhantomData};

use futures::Future;
use reth_primitives::{Receipt, Receipts};
use tokio::io::AsyncReadExt;
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};
use tracing::{trace, warn};

use crate::{DecodedFileChunk, FileClientError};

/// File client for reading RLP encoded receipts from file. Receipts in file must be in sequential
/// order w.r.t. block number.
#[derive(Debug)]
pub struct ReceiptFileClient<D> {
    /// The buffered receipts, read from file, as nested lists. One list per block number.
    pub receipts: Receipts,
    /// First (lowest) block number read from file.
    pub first_block: u64,
    /// Total number of receipts. Count of elements in [`Receipts`] flattened.
    pub total_receipts: usize,
    /// marker
    _marker: PhantomData<D>,
}

/// Constructs a file client from a reader and decoder.
pub trait FromReceiptReader<D> {
    /// Error returned by file client type.
    type Error: From<io::Error>;

    /// Returns a decoder instance
    fn decoder() -> D;

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

impl<D> FromReceiptReader<D> for ReceiptFileClient<D>
where
    D: Decoder<Item = Option<ReceiptWithBlockNumber>, Error = FileClientError>
        + fmt::Debug
        + Default,
{
    type Error = D::Error;

    fn decoder() -> D {
        D::default()
    }

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
        let mut receipts = Receipts::default();

        // use with_capacity to make sure the internal buffer contains the entire chunk
        let mut stream = FramedRead::with_capacity(reader, Self::decoder(), num_bytes as usize);

        trace!(target: "downloaders::file",
            target_num_bytes=num_bytes,
            capacity=stream.read_buffer().capacity(),
            codec=?Self::decoder(),
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

                match receipt {
                    Some(ReceiptWithBlockNumber { receipt, number }) => {
                        if block_number > number {
                            warn!(target: "downloaders::file", previous_block_number = block_number, "skipping receipt from a lower block: {number}");
                            continue
                        }

                        total_receipts += 1;

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
                    _marker: Default::default(),
                },
                remaining_bytes,
                highest_block: Some(block_number),
            })
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
    use alloy_primitives::{
        bytes::{Buf, BytesMut},
        hex, Address, Bytes, Log, LogData, B256,
    };
    use alloy_rlp::{Decodable, RlpDecodable};
    use reth_primitives::{Receipt, TxType};
    use reth_tracing::init_test_tracing;
    use tokio_util::codec::Decoder;

    use super::{FromReceiptReader, ReceiptFileClient, ReceiptWithBlockNumber};
    use crate::{DecodedFileChunk, FileClientError};

    #[derive(Debug, PartialEq, Eq, RlpDecodable)]
    struct MockReceipt {
        tx_type: u8,
        status: u64,
        cumulative_gas_used: u64,
        logs: Vec<Log>,
        block_number: u64,
    }

    #[derive(Debug, PartialEq, Eq, RlpDecodable)]
    #[rlp(trailing)]
    struct MockReceiptContainer(Option<MockReceipt>);

    impl TryFrom<MockReceipt> for ReceiptWithBlockNumber {
        type Error = &'static str;
        fn try_from(exported_receipt: MockReceipt) -> Result<Self, Self::Error> {
            let MockReceipt { tx_type, status, cumulative_gas_used, logs, block_number: number } =
                exported_receipt;

            #[allow(clippy::needless_update)]
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

    #[derive(Debug, Default)]
    struct MockReceiptFileCodec;

    impl Decoder for MockReceiptFileCodec {
        type Item = Option<ReceiptWithBlockNumber>;
        type Error = FileClientError;

        fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
            if src.is_empty() {
                return Ok(None)
            }

            let buf_slice = &mut src.as_ref();
            let receipt = MockReceiptContainer::decode(buf_slice)
                .map_err(|err| Self::Error::Rlp(err, src.to_vec()))?
                .0;
            src.advance(src.len() - buf_slice.len());

            Ok(Some(
                receipt
                    .map(|receipt| receipt.try_into().map_err(FileClientError::from))
                    .transpose()?,
            ))
        }
    }

    /// No receipts for genesis block
    const MOCK_RECEIPT_BLOCK_NO_TRANSACTIONS: &[u8] = &hex!("c0");

    const MOCK_RECEIPT_ENCODED_BLOCK_1: &[u8] = &hex!("f901a4f901a1800183031843f90197f89b948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef863a00109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac6027ba00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2da000000000000000000000000000000000000000000000000000000000618d8837f89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68ba000000000000000000000000000000000000000000000000000000000d0e3ebf0a00000000000000000000000000000000000000000000000000000000000014218a000000000000000000000000070b17c0fe982ab4a7ac17a4c25485643151a1f2d80f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234fa000000000000000000000000000000000000000000000007edc6ca0bb683480008001");

    const MOCK_RECEIPT_ENCODED_BLOCK_2: &[u8] = &hex!("f90106f9010380018301c60df8faf89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68da000000000000000000000000000000000000000000000000000000000d0ea0e40a00000000000000000000000000000000000000000000000000000000000014218a0000000000000000000000000e5e7492282fd1e3bfac337a0beccd29b15b7b24080f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234ea000000000000000000000000000000000000000000000007eda7867e0c7d480008002");

    const MOCK_RECEIPT_ENCODED_BLOCK_3: &[u8] = &hex!("f90106f9010380018301c60df8faf89c948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef884a092e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68da000000000000000000000000000000000000000000000000000000000d101e54ba00000000000000000000000000000000000000000000000000000000000014218a0000000000000000000000000fa011d8d6c26f13abe2cefed38226e401b2b8a9980f85a948ce8c13d816fe6daf12d6fd9e4952e1fc88850aef842a0fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234ea000000000000000000000000000000000000000000000007ed8842f06277480008003");

    fn mock_receipt_1() -> MockReceipt {
        let receipt = receipt_block_1();
        MockReceipt {
            tx_type: receipt.receipt.tx_type as u8,
            status: receipt.receipt.success as u64,

            cumulative_gas_used: receipt.receipt.cumulative_gas_used,
            logs: receipt.receipt.logs,
            block_number: 1,
        }
    }

    fn mock_receipt_2() -> MockReceipt {
        let receipt = receipt_block_2();
        MockReceipt {
            tx_type: receipt.receipt.tx_type as u8,
            status: receipt.receipt.success as u64,

            cumulative_gas_used: receipt.receipt.cumulative_gas_used,
            logs: receipt.receipt.logs,
            block_number: 2,
        }
    }

    fn mock_receipt_3() -> MockReceipt {
        let receipt = receipt_block_3();
        MockReceipt {
            tx_type: receipt.receipt.tx_type as u8,
            status: receipt.receipt.success as u64,

            cumulative_gas_used: receipt.receipt.cumulative_gas_used,
            logs: receipt.receipt.logs,
            block_number: 3,
        }
    }

    fn receipt_block_1() -> ReceiptWithBlockNumber {
        let log_1 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "0109fc6f55cf40689f02fbaad7af7fe7bbac8a3d2186600afc7d3e10cac6027b"
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
        };

        let log_2 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "92e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68b"
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
        };

        let log_3 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
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
        };

        // #[allow(clippy::needless_update)] not recognised, ..Default::default() needed so optimism
        // feature must not be brought into scope
        let mut receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 202819,
            ..Default::default()
        };
        receipt.logs = vec![log_1, log_2, log_3];

        ReceiptWithBlockNumber { receipt, number: 1 }
    }

    fn receipt_block_2() -> ReceiptWithBlockNumber {
        let log_1 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "92e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68d"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000000000000000000000000000000000000d0ea0e40"
                    )),
                    B256::from(hex!(
                        "0000000000000000000000000000000000000000000000000000000000014218"
                    )),
                    B256::from(hex!(
                        "000000000000000000000000e5e7492282fd1e3bfac337a0beccd29b15b7b240"
                    )),
                ],
                Bytes::default(),
            )
            .unwrap(),
        };

        let log_2 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234e"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000000000000000000000000007eda7867e0c7d48000"
                    )),
                ],
                Bytes::default(),
            )
            .unwrap(),
        };

        // #[allow(clippy::needless_update)] not recognised, ..Default::default() needed so optimism
        // feature must not be brought into scope
        let mut receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 116237,
            ..Default::default()
        };
        receipt.logs = vec![log_1, log_2];

        ReceiptWithBlockNumber { receipt, number: 2 }
    }

    fn receipt_block_3() -> ReceiptWithBlockNumber {
        let log_1 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "92e98423f8adac6e64d0608e519fd1cefb861498385c6dee70d58fc926ddc68d"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000000000000000000000000000000000000d101e54b"
                    )),
                    B256::from(hex!(
                        "0000000000000000000000000000000000000000000000000000000000014218"
                    )),
                    B256::from(hex!(
                        "000000000000000000000000fa011d8d6c26f13abe2cefed38226e401b2b8a99"
                    )),
                ],
                Bytes::default(),
            )
            .unwrap(),
        };

        let log_2 = Log {
            address: Address::from(hex!("8ce8c13d816fe6daf12d6fd9e4952e1fc88850ae")),
            data: LogData::new(
                vec![
                    B256::from(hex!(
                        "fe25c73e3b9089fac37d55c4c7efcba6f04af04cebd2fc4d6d7dbb07e1e5234e"
                    )),
                    B256::from(hex!(
                        "00000000000000000000000000000000000000000000007ed8842f0627748000"
                    )),
                ],
                Bytes::default(),
            )
            .unwrap(),
        };

        // #[allow(clippy::needless_update)] not recognised, ..Default::default() needed so optimism
        // feature must not be brought into scope
        let mut receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 116237,
            ..Default::default()
        };
        receipt.logs = vec![log_1, log_2];

        ReceiptWithBlockNumber { receipt, number: 3 }
    }

    #[test]
    fn decode_mock_receipt() {
        let receipt1 = mock_receipt_1();
        let decoded1 = MockReceiptContainer::decode(&mut &MOCK_RECEIPT_ENCODED_BLOCK_1[..])
            .unwrap()
            .0
            .unwrap();
        assert_eq!(receipt1, decoded1);

        let receipt2 = mock_receipt_2();
        let decoded2 = MockReceiptContainer::decode(&mut &MOCK_RECEIPT_ENCODED_BLOCK_2[..])
            .unwrap()
            .0
            .unwrap();
        assert_eq!(receipt2, decoded2);

        let receipt3 = mock_receipt_3();
        let decoded3 = MockReceiptContainer::decode(&mut &MOCK_RECEIPT_ENCODED_BLOCK_3[..])
            .unwrap()
            .0
            .unwrap();
        assert_eq!(receipt3, decoded3);
    }

    #[test]
    #[allow(clippy::needless_update)]
    fn receipts_codec() {
        // rig

        let mut receipt_1_to_3 = MOCK_RECEIPT_ENCODED_BLOCK_1.to_vec();
        receipt_1_to_3.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_2);
        receipt_1_to_3.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_3);

        let encoded = &mut BytesMut::from(&receipt_1_to_3[..]);

        let mut codec = MockReceiptFileCodec;

        // test

        let first_decoded_receipt = codec.decode(encoded).unwrap().unwrap().unwrap();

        assert_eq!(receipt_block_1(), first_decoded_receipt);

        let second_decoded_receipt = codec.decode(encoded).unwrap().unwrap().unwrap();

        assert_eq!(receipt_block_2(), second_decoded_receipt);

        let third_decoded_receipt = codec.decode(encoded).unwrap().unwrap().unwrap();

        assert_eq!(receipt_block_3(), third_decoded_receipt);
    }

    #[tokio::test]
    async fn receipt_file_client_ovm_codec() {
        init_test_tracing();

        // genesis block has no hack receipts
        let mut encoded_receipts = MOCK_RECEIPT_BLOCK_NO_TRANSACTIONS.to_vec();
        // one receipt each for block 1 and 2
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_1);
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_2);
        // no receipt for block 4
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_BLOCK_NO_TRANSACTIONS);

        let encoded_byte_len = encoded_receipts.len() as u64;
        let reader = &mut &encoded_receipts[..];

        let DecodedFileChunk {
            file_client: ReceiptFileClient { receipts, first_block, total_receipts, .. },
            ..
        } = ReceiptFileClient::<MockReceiptFileCodec>::from_receipt_reader(
            reader,
            encoded_byte_len,
            None,
        )
        .await
        .unwrap();

        // 2 non-empty receipt objects
        assert_eq!(2, total_receipts);
        assert_eq!(0, first_block);
        assert!(receipts[0].is_empty());
        assert_eq!(receipt_block_1().receipt, receipts[1][0].clone().unwrap());
        assert_eq!(receipt_block_2().receipt, receipts[2][0].clone().unwrap());
        assert!(receipts[3].is_empty());
    }

    #[tokio::test]
    async fn no_receipts_middle_block() {
        init_test_tracing();

        // genesis block has no hack receipts
        let mut encoded_receipts = MOCK_RECEIPT_BLOCK_NO_TRANSACTIONS.to_vec();
        // one receipt each for block 1
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_1);
        // no receipt for block 2
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_BLOCK_NO_TRANSACTIONS);
        // one receipt for block 3
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_3);

        let encoded_byte_len = encoded_receipts.len() as u64;
        let reader = &mut &encoded_receipts[..];

        let DecodedFileChunk {
            file_client: ReceiptFileClient { receipts, first_block, total_receipts, .. },
            ..
        } = ReceiptFileClient::<MockReceiptFileCodec>::from_receipt_reader(
            reader,
            encoded_byte_len,
            None,
        )
        .await
        .unwrap();

        // 2 non-empty receipt objects
        assert_eq!(2, total_receipts);
        assert_eq!(0, first_block);
        assert!(receipts[0].is_empty());
        assert_eq!(receipt_block_1().receipt, receipts[1][0].clone().unwrap());
        assert!(receipts[2].is_empty());
        assert_eq!(receipt_block_3().receipt, receipts[3][0].clone().unwrap());
    }

    #[tokio::test]
    async fn two_receipts_same_block() {
        init_test_tracing();

        // genesis block has no hack receipts
        let mut encoded_receipts = MOCK_RECEIPT_BLOCK_NO_TRANSACTIONS.to_vec();
        // one receipt each for block 1
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_1);
        // two receipts for block 2
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_2);
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_2);
        // one receipt for block 3
        encoded_receipts.extend_from_slice(MOCK_RECEIPT_ENCODED_BLOCK_3);

        let encoded_byte_len = encoded_receipts.len() as u64;
        let reader = &mut &encoded_receipts[..];

        let DecodedFileChunk {
            file_client: ReceiptFileClient { receipts, first_block, total_receipts, .. },
            ..
        } = ReceiptFileClient::<MockReceiptFileCodec>::from_receipt_reader(
            reader,
            encoded_byte_len,
            None,
        )
        .await
        .unwrap();

        // 4 non-empty receipt objects
        assert_eq!(4, total_receipts);
        assert_eq!(0, first_block);
        assert!(receipts[0].is_empty());
        assert_eq!(receipt_block_1().receipt, receipts[1][0].clone().unwrap());
        assert_eq!(receipt_block_2().receipt, receipts[2][0].clone().unwrap());
        assert_eq!(receipt_block_2().receipt, receipts[2][1].clone().unwrap());
        assert_eq!(receipt_block_3().receipt, receipts[3][0].clone().unwrap());
    }
}
