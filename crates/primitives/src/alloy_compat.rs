//! Common conversions from alloy types.

use crate::{Block, BlockBody, Transaction, TransactionSigned};
use alloc::{string::ToString, vec::Vec};
use alloy_consensus::{constants::EMPTY_TRANSACTIONS, Header, TxEnvelope};
use alloy_network::{AnyHeader, AnyRpcBlock, AnyRpcTransaction, AnyTxEnvelope};
use alloy_serde::WithOtherFields;
use op_alloy_rpc_types as _;

impl TryFrom<AnyRpcBlock> for Block {
    type Error = alloy_rpc_types::ConversionError;

    fn try_from(block: AnyRpcBlock) -> Result<Self, Self::Error> {
        use alloy_rpc_types::ConversionError;

        let block = block.inner;

        let transactions = {
            let transactions: Result<Vec<TransactionSigned>, ConversionError> = match block
                .transactions
            {
                alloy_rpc_types::BlockTransactions::Full(transactions) => {
                    transactions.into_iter().map(|tx| tx.try_into()).collect()
                }
                alloy_rpc_types::BlockTransactions::Hashes(_) |
                alloy_rpc_types::BlockTransactions::Uncle => {
                    // alloy deserializes empty blocks into `BlockTransactions::Hashes`, if the tx
                    // root is the empty root then we can just return an empty vec.
                    if block.header.transactions_root == EMPTY_TRANSACTIONS {
                        Ok(Vec::new())
                    } else {
                        Err(ConversionError::Custom("missing transactions".to_string()))
                    }
                }
            };
            transactions?
        };

        let AnyHeader {
            parent_hash,
            ommers_hash,
            beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            logs_bloom,
            difficulty,
            number,
            gas_limit,
            gas_used,
            timestamp,
            extra_data,
            mix_hash,
            nonce,
            base_fee_per_gas,
            withdrawals_root,
            blob_gas_used,
            excess_blob_gas,
            parent_beacon_block_root,
            requests_hash,
            target_blobs_per_block,
        } = block.header.inner;

        Ok(Self {
            header: Header {
                parent_hash,
                ommers_hash,
                beneficiary,
                state_root,
                transactions_root,
                receipts_root,
                logs_bloom,
                difficulty,
                number,
                gas_limit,
                gas_used,
                timestamp,
                extra_data,
                mix_hash: mix_hash
                    .ok_or_else(|| ConversionError::Custom("missing mixHash".to_string()))?,
                nonce: nonce.ok_or_else(|| ConversionError::Custom("missing nonce".to_string()))?,
                base_fee_per_gas,
                withdrawals_root,
                blob_gas_used,
                excess_blob_gas,
                parent_beacon_block_root,
                requests_hash,
                target_blobs_per_block,
            },
            body: BlockBody {
                transactions,
                ommers: Default::default(),
                withdrawals: block.withdrawals.map(|w| w.into_inner().into()),
            },
        })
    }
}

impl TryFrom<AnyRpcTransaction> for TransactionSigned {
    type Error = alloy_rpc_types::ConversionError;

    fn try_from(tx: AnyRpcTransaction) -> Result<Self, Self::Error> {
        use alloy_rpc_types::ConversionError;

        let WithOtherFields { inner: tx, other: _ } = tx;

        let (transaction, signature, hash) = match tx.inner {
            AnyTxEnvelope::Ethereum(TxEnvelope::Legacy(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Legacy(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip2930(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip2930(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip1559(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip1559(tx), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip4844(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip4844(tx.into()), signature, hash)
            }
            AnyTxEnvelope::Ethereum(TxEnvelope::Eip7702(tx)) => {
                let (tx, signature, hash) = tx.into_parts();
                (Transaction::Eip7702(tx), signature, hash)
            }
            #[cfg(feature = "optimism")]
            AnyTxEnvelope::Unknown(alloy_network::UnknownTxEnvelope { hash, inner }) => {
                use alloy_consensus::Transaction as _;

                if inner.ty() == crate::TxType::Deposit {
                    let fields: op_alloy_rpc_types::OpTransactionFields = inner
                        .fields
                        .clone()
                        .deserialize_into::<op_alloy_rpc_types::OpTransactionFields>()
                        .map_err(|e| ConversionError::Custom(e.to_string()))?;
                    (
                        Transaction::Deposit(op_alloy_consensus::TxDeposit {
                            source_hash: fields.source_hash.ok_or_else(|| {
                                ConversionError::Custom("MissingSourceHash".to_string())
                            })?,
                            from: tx.from,
                            to: revm_primitives::TxKind::from(inner.to()),
                            mint: fields.mint.filter(|n| *n != 0),
                            value: inner.value(),
                            gas_limit: inner.gas_limit(),
                            is_system_transaction: fields.is_system_tx.unwrap_or(false),
                            input: inner.input().clone(),
                        }),
                        op_alloy_consensus::TxDeposit::signature(),
                        hash,
                    )
                } else {
                    return Err(ConversionError::Custom("unknown transaction type".to_string()))
                }
            }
            _ => return Err(ConversionError::Custom("unknown transaction type".to_string())),
        };

        Ok(Self { transaction, signature, hash: hash.into() })
    }
}

#[cfg(test)]
#[cfg(feature = "optimism")]
mod tests {
    use super::*;
    use alloy_primitives::{address, Address, B256, U256};
    use revm_primitives::TxKind;

    #[test]
    fn optimism_deposit_tx_conversion_no_mint() {
        let input = r#"{
            "blockHash": "0xef664d656f841b5ad6a2b527b963f1eb48b97d7889d742f6cbff6950388e24cd",
            "blockNumber": "0x73a78fd",
            "depositReceiptVersion": "0x1",
            "from": "0x36bde71c97b33cc4729cf772ae268934f7ab70b2",
            "gas": "0xc27a8",
            "gasPrice": "0x0",
            "hash": "0x0bf1845c5d7a82ec92365d5027f7310793d53004f3c86aa80965c67bf7e7dc80",
            "input": "0xd764ad0b000100000000000000000000000000000000000000000000000000000001cf5400000000000000000000000099c9fc46f92e8a1c0dec1b1747d010903e884be100000000000000000000000042000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000007a12000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000000e40166a07a0000000000000000000000000994206dfe8de6ec6920ff4d779b0d950605fb53000000000000000000000000d533a949740bb3306d119cc777fa900ba034cd52000000000000000000000000ca74f404e0c7bfa35b13b511097df966d5a65597000000000000000000000000ca74f404e0c7bfa35b13b511097df966d5a65597000000000000000000000000000000000000000000000216614199391dbba2ba00000000000000000000000000000000000000000000000000000000000000c0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "mint": "0x0",
            "nonce": "0x74060",
            "r": "0x0",
            "s": "0x0",
            "sourceHash": "0x074adb22f2e6ed9bdd31c52eefc1f050e5db56eb85056450bccd79a6649520b3",
            "to": "0x4200000000000000000000000000000000000007",
            "transactionIndex": "0x1",
            "type": "0x7e",
            "v": "0x0",
            "value": "0x0"
        }"#;
        let alloy_tx: WithOtherFields<alloy_rpc_types::Transaction<AnyTxEnvelope>> =
            serde_json::from_str(input).expect("failed to deserialize");

        let TransactionSigned { transaction: reth_tx, .. } =
            alloy_tx.try_into().expect("failed to convert");
        if let Transaction::Deposit(deposit_tx) = reth_tx {
            assert_eq!(
                deposit_tx.source_hash,
                "0x074adb22f2e6ed9bdd31c52eefc1f050e5db56eb85056450bccd79a6649520b3"
                    .parse::<B256>()
                    .unwrap()
            );
            assert_eq!(
                deposit_tx.from,
                "0x36bde71c97b33cc4729cf772ae268934f7ab70b2".parse::<Address>().unwrap()
            );
            assert_eq!(
                deposit_tx.to,
                TxKind::from(address!("4200000000000000000000000000000000000007"))
            );
            assert_eq!(deposit_tx.mint, None);
            assert_eq!(deposit_tx.value, U256::ZERO);
            assert_eq!(deposit_tx.gas_limit, 796584);
            assert!(!deposit_tx.is_system_transaction);
        } else {
            panic!("Expected Deposit transaction");
        }
    }

    #[test]
    fn optimism_deposit_tx_conversion_mint() {
        let input = r#"{
            "blockHash": "0x7194f63b105e93fb1a27c50d23d62e422d4185a68536c55c96284911415699b2",
            "blockNumber": "0x73a82cc",
            "depositReceiptVersion": "0x1",
            "from": "0x36bde71c97b33cc4729cf772ae268934f7ab70b2",
            "gas": "0x7812e",
            "gasPrice": "0x0",
            "hash": "0xf7e83886d3c6864f78e01c453ebcd57020c5795d96089e8f0e0b90a467246ddb",
            "input": "0xd764ad0b000100000000000000000000000000000000000000000000000000000001cf5f00000000000000000000000099c9fc46f92e8a1c0dec1b1747d010903e884be100000000000000000000000042000000000000000000000000000000000000100000000000000000000000000000000000000000000000239c2e16a5ca5900000000000000000000000000000000000000000000000000000000000000030d4000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000000e41635f5fd0000000000000000000000002ce910fbba65b454bbaf6a18c952a70f3bcd82990000000000000000000000002ce910fbba65b454bbaf6a18c952a70f3bcd82990000000000000000000000000000000000000000000000239c2e16a5ca590000000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000040000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
            "mint": "0x239c2e16a5ca590000",
            "nonce": "0x7406b",
            "r": "0x0",
            "s": "0x0",
            "sourceHash": "0xe0358cd2b2686d297c5c859646a613124a874fb9d9c4a2c88636a46a65c06e48",
            "to": "0x4200000000000000000000000000000000000007",
            "transactionIndex": "0x1",
            "type": "0x7e",
            "v": "0x0",
            "value": "0x239c2e16a5ca590000"
        }"#;
        let alloy_tx: WithOtherFields<alloy_rpc_types::Transaction<AnyTxEnvelope>> =
            serde_json::from_str(input).expect("failed to deserialize");

        let TransactionSigned { transaction: reth_tx, .. } =
            alloy_tx.try_into().expect("failed to convert");

        if let Transaction::Deposit(deposit_tx) = reth_tx {
            assert_eq!(
                deposit_tx.source_hash,
                "0xe0358cd2b2686d297c5c859646a613124a874fb9d9c4a2c88636a46a65c06e48"
                    .parse::<B256>()
                    .unwrap()
            );
            assert_eq!(
                deposit_tx.from,
                "0x36bde71c97b33cc4729cf772ae268934f7ab70b2".parse::<Address>().unwrap()
            );
            assert_eq!(
                deposit_tx.to,
                TxKind::from(address!("4200000000000000000000000000000000000007"))
            );
            assert_eq!(deposit_tx.mint, Some(656890000000000000000));
            assert_eq!(deposit_tx.value, U256::from(0x239c2e16a5ca590000_u128));
            assert_eq!(deposit_tx.gas_limit, 491822);
            assert!(!deposit_tx.is_system_transaction);
        } else {
            panic!("Expected Deposit transaction");
        }
    }
}
