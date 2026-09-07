#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_consensus::Transaction;
use alloy_primitives::{Address, Bytes, B256};
use reth_ethereum_primitives::{Block, Receipt};
use reth_primitives_traits::RecoveredBlock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptsSsz(Vec<ReceiptSsz>);

impl ReceiptsSsz {
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, index: usize) -> Option<&ReceiptSsz> {
        self.0.get(index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptSsz {
    tx_type: u8,
    success: bool,
    gas_used: u64,
    contract_address: Option<Address>,
    logs: Vec<LogSsz>,
}

impl ReceiptSsz {
    pub fn logs(&self) -> &[LogSsz] {
        &self.logs
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSsz {
    address: Address,
    topics: Vec<B256>,
    data: Bytes,
}

impl LogSsz {
    pub const fn address(&self) -> Address {
        self.address
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptConversionError {
    ReceiptCountMismatch { transactions: usize, receipts: usize },
    TransactionTypeMismatch { index: usize, transaction: u8, receipt: u8 },
    DecreasingCumulativeGas { index: usize, previous: u64, current: u64 },
    TooManyTopics { receipt_index: usize, log_index: usize, actual: usize },
}

const MAX_TOPICS: usize = 4;

pub fn convert_receipts(
    block: &RecoveredBlock<Block>,
    receipts: &[Receipt],
) -> Result<ReceiptsSsz, ReceiptConversionError> {
    let transaction_count = block.body().transactions.len();
    if transaction_count != receipts.len() {
        return Err(ReceiptConversionError::ReceiptCountMismatch {
            transactions: transaction_count,
            receipts: receipts.len(),
        });
    }

    let mut previous_cumulative_gas_used = 0;
    let mut converted = Vec::with_capacity(receipts.len());

    for (index, (transaction, receipt)) in block.transactions_recovered().zip(receipts).enumerate()
    {
        let transaction_type = transaction.tx_type() as u8;
        let receipt_type = receipt.tx_type as u8;
        if transaction_type != receipt_type {
            return Err(ReceiptConversionError::TransactionTypeMismatch {
                index,
                transaction: transaction_type,
                receipt: receipt_type,
            });
        }

        let current_cumulative_gas_used = receipt.cumulative_gas_used;
        let gas_used = if index == 0 {
            current_cumulative_gas_used
        } else {
            current_cumulative_gas_used.checked_sub(previous_cumulative_gas_used).ok_or(
                ReceiptConversionError::DecreasingCumulativeGas {
                    index,
                    previous: previous_cumulative_gas_used,
                    current: current_cumulative_gas_used,
                },
            )?
        };
        previous_cumulative_gas_used = current_cumulative_gas_used;

        let contract_address = (receipt.success && transaction.is_create())
            .then(|| transaction.signer().create(transaction.nonce()));

        let mut logs = Vec::with_capacity(receipt.logs.len());
        for (log_index, log) in receipt.logs.iter().enumerate() {
            let topics = log.data.topics();
            if topics.len() > MAX_TOPICS {
                return Err(ReceiptConversionError::TooManyTopics {
                    receipt_index: index,
                    log_index,
                    actual: topics.len(),
                });
            }

            logs.push(LogSsz {
                address: log.address,
                topics: topics.to_vec(),
                data: log.data.data.clone(),
            });
        }

        converted.push(ReceiptSsz {
            tx_type: receipt_type,
            success: receipt.success,
            gas_used,
            contract_address,
            logs,
        });
    }

    Ok(ReceiptsSsz(converted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{TxEip1559, TxLegacy, TxType};
    use alloy_primitives::{Log, Signature, TxKind};
    use reth_ethereum_primitives::{
        BlockBody, Transaction as EthereumTransaction, TransactionSigned,
    };

    fn block(transactions: Vec<TransactionSigned>, senders: Vec<Address>) -> RecoveredBlock<Block> {
        assert_eq!(transactions.len(), senders.len());
        RecoveredBlock::try_new_unhashed(
            Block {
                header: Default::default(),
                body: BlockBody { transactions, ..Default::default() },
            },
            senders,
        )
        .unwrap()
    }

    fn legacy_transaction(nonce: u64, to: TxKind) -> TransactionSigned {
        TransactionSigned::new_unhashed(
            EthereumTransaction::Legacy(TxLegacy { nonce, to, ..Default::default() }),
            Signature::test_signature(),
        )
    }

    fn eip1559_transaction(nonce: u64, to: TxKind) -> TransactionSigned {
        TransactionSigned::new_unhashed(
            EthereumTransaction::Eip1559(TxEip1559 { nonce, to, ..Default::default() }),
            Signature::test_signature(),
        )
    }

    fn receipt(
        tx_type: TxType,
        success: bool,
        cumulative_gas_used: u64,
        logs: Vec<Log>,
    ) -> Receipt {
        Receipt { tx_type, success, cumulative_gas_used, logs }
    }

    fn log(address: Address, topics: Vec<B256>, data: &[u8]) -> Log {
        Log::new_unchecked(address, topics, Bytes::from(data.to_vec()))
    }

    #[test]
    fn empty_receipt_list() {
        let block = block(Vec::new(), Vec::new());

        let converted = convert_receipts(&block, &[]).unwrap();

        assert!(converted.is_empty());
        assert_eq!(converted.len(), 0);
    }

    #[test]
    fn one_receipt_with_one_log() {
        let sender = Address::repeat_byte(0x11);
        let address = Address::repeat_byte(0x22);
        let topics = vec![B256::repeat_byte(0x33), B256::repeat_byte(0x44)];
        let data = [0x00, 0x01, 0x7f, 0x80, 0xff, 0x00];
        let block = block(
            vec![eip1559_transaction(0, TxKind::Call(Address::repeat_byte(0x55)))],
            vec![sender],
        );
        let receipts =
            [receipt(TxType::Eip1559, true, 21_000, vec![log(address, topics.clone(), &data)])];

        let converted = convert_receipts(&block, &receipts).unwrap();
        let converted_receipt = converted.get(0).unwrap();
        let converted_log = &converted_receipt.logs[0];

        assert_eq!(converted.len(), 1);
        assert_eq!(converted_receipt.tx_type, TxType::Eip1559 as u8);
        assert!(converted_receipt.success);
        assert_eq!(converted_receipt.gas_used, 21_000);
        assert_eq!(converted_receipt.contract_address, None);
        assert_eq!(converted_log.address, address);
        assert_eq!(converted_log.topics, topics);
        assert_eq!(converted_log.data, Bytes::from(data.to_vec()));
    }

    #[test]
    fn multiple_receipts_preserve_order() {
        let sender = Address::repeat_byte(0x11);
        let first_address = Address::repeat_byte(0x21);
        let second_address = Address::repeat_byte(0x22);
        let block = block(
            vec![
                legacy_transaction(0, TxKind::Call(Address::ZERO)),
                legacy_transaction(1, TxKind::Call(Address::ZERO)),
            ],
            vec![sender, sender],
        );
        let receipts = [
            receipt(TxType::Legacy, true, 21_000, vec![log(first_address, vec![], &[])]),
            receipt(TxType::Legacy, true, 42_000, vec![log(second_address, vec![], &[])]),
        ];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().logs[0].address, first_address);
        assert_eq!(converted.get(1).unwrap().logs[0].address, second_address);
    }

    #[test]
    fn multiple_logs_preserve_order() {
        let sender = Address::repeat_byte(0x11);
        let addresses =
            [Address::repeat_byte(0x21), Address::repeat_byte(0x22), Address::repeat_byte(0x23)];
        let block = block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![sender]);
        let receipts = [receipt(
            TxType::Legacy,
            true,
            21_000,
            addresses.into_iter().map(|address| log(address, vec![], &[])).collect(),
        )];

        let converted = convert_receipts(&block, &receipts).unwrap();
        let converted_logs = &converted.get(0).unwrap().logs;

        assert_eq!(converted_logs[0].address, addresses[0]);
        assert_eq!(converted_logs[1].address, addresses[1]);
        assert_eq!(converted_logs[2].address, addresses[2]);
    }

    #[test]
    fn changed_log_address_remains_distinguishable() {
        let sender = Address::repeat_byte(0x11);
        let first_address = Address::repeat_byte(0x21);
        let second_address = Address::repeat_byte(0x22);
        let block = block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![sender]);
        let first = [receipt(TxType::Legacy, true, 21_000, vec![log(first_address, vec![], &[])])];
        let second =
            [receipt(TxType::Legacy, true, 21_000, vec![log(second_address, vec![], &[])])];

        let first = convert_receipts(&block, &first).unwrap();
        let second = convert_receipts(&block, &second).unwrap();

        assert_ne!(first.get(0).unwrap().logs[0].address, second.get(0).unwrap().logs[0].address);
    }

    #[test]
    fn four_topics_are_accepted() {
        let topics = vec![
            B256::repeat_byte(0x01),
            B256::repeat_byte(0x02),
            B256::repeat_byte(0x03),
            B256::repeat_byte(0x04),
        ];
        let block =
            block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![Address::ZERO]);
        let receipts =
            [receipt(TxType::Legacy, true, 21_000, vec![log(Address::ZERO, topics.clone(), &[])])];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().logs[0].topics, topics);
    }

    #[test]
    fn too_many_topics_are_rejected() {
        let topics = (1..=5).map(B256::repeat_byte).collect();
        let block =
            block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![Address::ZERO]);
        let receipts =
            [receipt(TxType::Legacy, true, 21_000, vec![log(Address::ZERO, topics, &[])])];

        let error = convert_receipts(&block, &receipts).unwrap_err();

        assert_eq!(
            error,
            ReceiptConversionError::TooManyTopics { receipt_index: 0, log_index: 0, actual: 5 }
        );
    }

    #[test]
    fn first_receipt_uses_cumulative_gas() {
        let block =
            block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![Address::ZERO]);
        let receipts = [receipt(TxType::Legacy, true, 21_000, vec![])];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().gas_used, 21_000);
    }

    #[test]
    fn later_receipt_uses_cumulative_difference() {
        let block = block(
            vec![
                legacy_transaction(0, TxKind::Call(Address::ZERO)),
                legacy_transaction(1, TxKind::Call(Address::ZERO)),
            ],
            vec![Address::ZERO; 2],
        );
        let receipts = [
            receipt(TxType::Legacy, true, 21_000, vec![]),
            receipt(TxType::Legacy, true, 71_000, vec![]),
        ];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().gas_used, 21_000);
        assert_eq!(converted.get(1).unwrap().gas_used, 50_000);
    }

    #[test]
    fn decreasing_cumulative_gas_is_rejected() {
        let block = block(
            vec![
                legacy_transaction(0, TxKind::Call(Address::ZERO)),
                legacy_transaction(1, TxKind::Call(Address::ZERO)),
            ],
            vec![Address::ZERO; 2],
        );
        let receipts = [
            receipt(TxType::Legacy, true, 50_000, vec![]),
            receipt(TxType::Legacy, true, 40_000, vec![]),
        ];

        let error = convert_receipts(&block, &receipts).unwrap_err();

        assert_eq!(
            error,
            ReceiptConversionError::DecreasingCumulativeGas {
                index: 1,
                previous: 50_000,
                current: 40_000,
            }
        );
    }

    #[test]
    fn successful_creation_has_contract_address() {
        let sender = Address::repeat_byte(0x11);
        let nonce = 7;
        let block = block(vec![legacy_transaction(nonce, TxKind::Create)], vec![sender]);
        let receipts = [receipt(TxType::Legacy, true, 53_000, vec![])];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().contract_address, Some(sender.create(nonce)));
    }

    #[test]
    fn failed_creation_has_no_contract_address() {
        let block =
            block(vec![legacy_transaction(7, TxKind::Create)], vec![Address::repeat_byte(0x11)]);
        let receipts = [receipt(TxType::Legacy, false, 53_000, vec![])];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().contract_address, None);
    }

    #[test]
    fn successful_call_has_no_contract_address() {
        let block = block(
            vec![legacy_transaction(7, TxKind::Call(Address::ZERO))],
            vec![Address::repeat_byte(0x11)],
        );
        let receipts = [receipt(TxType::Legacy, true, 21_000, vec![])];

        let converted = convert_receipts(&block, &receipts).unwrap();

        assert_eq!(converted.get(0).unwrap().contract_address, None);
    }

    #[test]
    fn receipt_transaction_count_mismatch_is_rejected() {
        let block =
            block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![Address::ZERO]);

        let error = convert_receipts(&block, &[]).unwrap_err();

        assert_eq!(
            error,
            ReceiptConversionError::ReceiptCountMismatch { transactions: 1, receipts: 0 }
        );
    }

    #[test]
    fn transaction_type_mismatch_is_rejected() {
        let block =
            block(vec![legacy_transaction(0, TxKind::Call(Address::ZERO))], vec![Address::ZERO]);
        let receipts = [receipt(TxType::Eip1559, true, 21_000, vec![])];

        let error = convert_receipts(&block, &receipts).unwrap_err();

        assert_eq!(
            error,
            ReceiptConversionError::TransactionTypeMismatch {
                index: 0,
                transaction: TxType::Legacy as u8,
                receipt: TxType::Eip1559 as u8,
            }
        );
    }
}
