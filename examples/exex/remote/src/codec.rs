use std::sync::Arc;

use eyre::OptionExt;
use reth::primitives::{Address, BlockHash, Bloom, TxHash, B256, U256};

use crate::proto;

pub fn to_proto_notification(
    notification: &reth_exex::ExExNotification,
) -> eyre::Result<proto::ExExNotification> {
    let notification = match notification {
        reth_exex::ExExNotification::ChainCommitted { new } => {
            proto::ex_ex_notification::Notification::ChainCommitted(proto::ChainCommitted {
                new: Some(to_proto_chain(&new)?),
            })
        }
        reth_exex::ExExNotification::ChainReorged { old, new } => {
            proto::ex_ex_notification::Notification::ChainReorged(proto::ChainReorged {
                old: Some(to_proto_chain(&old)?),
                new: Some(to_proto_chain(&new)?),
            })
        }
        reth_exex::ExExNotification::ChainReverted { old } => {
            proto::ex_ex_notification::Notification::ChainReverted(proto::ChainReverted {
                old: Some(to_proto_chain(&old)?),
            })
        }
    };

    Ok(proto::ExExNotification { notification: Some(notification) })
}

pub fn to_proto_chain(chain: &reth::providers::Chain) -> eyre::Result<proto::Chain> {
    let bundle_state = chain.execution_outcome().state();
    Ok(proto::Chain {
        blocks: chain
            .blocks_iter()
            .map(|block| proto::Block {
                header: Some(proto::SealedHeader {
                    hash: block.header.hash().to_vec(),
                    header: Some(to_proto_header(&block.header.header())),
                }),
                body: block.transactions().map(to_proto_transaction).collect(),
                ommers: block.ommers.iter().map(to_proto_header).collect(),
                senders: block.senders.iter().map(|sender| sender.to_vec()).collect(),
            })
            .collect(),
        execution_outcome: Some(proto::ExecutionOutcome {
            bundle: Some(proto::BundleState {
                state: bundle_state
                    .state
                    .iter()
                    .map(|(address, account)| {
                        Ok(proto::BundleAccount {
                            address: address.to_vec(),
                            info: account.info.as_ref().map(to_proto_account_info).transpose()?,
                            original_info: account
                                .original_info
                                .as_ref()
                                .map(to_proto_account_info)
                                .transpose()?,
                            storage: account
                                .storage
                                .iter()
                                .map(|(key, slot)| proto::StorageSlot {
                                    key: key.to_le_bytes_vec(),
                                    previous_or_original_value: slot
                                        .previous_or_original_value
                                        .to_le_bytes_vec(),
                                    present_value: slot.present_value.to_le_bytes_vec(),
                                })
                                .collect(),
                            status: to_proto_account_status(account.status) as i32,
                        })
                    })
                    .collect::<eyre::Result<_>>()?,
                contracts: bundle_state
                    .contracts
                    .iter()
                    .map(|(hash, bytecode)| {
                        Ok(proto::ContractBytecode {
                            hash: hash.to_vec(),
                            bytecode: Some(to_proto_bytecode(bytecode)?),
                        })
                    })
                    .collect::<eyre::Result<_>>()?,
                reverts: bundle_state
                    .reverts
                    .iter()
                    .map(|block_reverts| Ok(proto::BlockReverts {
                        reverts: block_reverts
                            .iter()
                            .map(|(address, revert)| Ok(proto::Revert {
                                address: address.to_vec(),
                                account: Some(proto::AccountInfoRevert { revert: Some(match &revert.account {
                                    reth::revm::db::states::reverts::AccountInfoRevert::DoNothing => proto::account_info_revert::Revert::DoNothing(()),
                                    reth::revm::db::states::reverts::AccountInfoRevert::DeleteIt => proto::account_info_revert::Revert::DeleteIt(()),
                                    reth::revm::db::states::reverts::AccountInfoRevert::RevertTo(account_info) => proto::account_info_revert::Revert::RevertTo(to_proto_account_info(account_info)?),
                                })}),
                                storage: revert.storage.iter().map(|(key, slot)| Ok(proto::RevertToSlot {
                                    key: key.to_le_bytes_vec(),
                                    revert: Some(match slot {
                                        reth::revm::db::RevertToSlot::Some(value) => proto::revert_to_slot::Revert::Some(value.to_le_bytes_vec()),
                                        reth::revm::db::RevertToSlot::Destroyed => proto::revert_to_slot::Revert::Destroyed(()),
                                    }),
                                })).collect::<eyre::Result<_>>()?,
                                previous_status: to_proto_account_status(revert.previous_status) as i32,
                                wipe_storage: revert.wipe_storage,
                            }))
                            .collect::<eyre::Result<_>>()?,
                    }))
                    .collect::<eyre::Result<_>>()?,
                state_size: bundle_state.state_size as u64,
                reverts_size: bundle_state.reverts_size as u64,
            }),
            receipts: chain
                .execution_outcome()
                .receipts()
                .iter()
                .map(|block_receipts| proto::BlockReceipts {
                    receipts: block_receipts
                        .iter()
                        .map(|receipt| proto::Receipt {
                            receipt: Some(receipt.as_ref().map_or(
                                proto::receipt::Receipt::Empty(()),
                                |receipt| {
                                    proto::receipt::Receipt::NonEmpty(proto::NonEmptyReceipt {
                                        tx_type: match receipt.tx_type {
                                            reth::primitives::TxType::Legacy => {
                                                proto::TxType::Legacy
                                            }
                                            reth::primitives::TxType::Eip2930 => {
                                                proto::TxType::Eip2930
                                            }
                                            reth::primitives::TxType::Eip1559 => {
                                                proto::TxType::Eip1559
                                            }
                                            reth::primitives::TxType::Eip4844 => {
                                                proto::TxType::Eip4844
                                            }
                                        } as i32,
                                        success: receipt.success,
                                        cumulative_gas_used: receipt.cumulative_gas_used,
                                        logs: receipt
                                            .logs
                                            .iter()
                                            .map(|log| proto::Log {
                                                address: log.address.to_vec(),
                                                data: Some(proto::LogData {
                                                    topics: log
                                                        .data
                                                        .topics()
                                                        .iter()
                                                        .map(|topic| topic.to_vec())
                                                        .collect(),
                                                    data: log.data.data.to_vec(),
                                                }),
                                            })
                                            .collect(),
                                    })
                                },
                            )),
                        })
                        .collect(),
                })
                .collect(),
            first_block: chain.execution_outcome().first_block,
        }),
    })
}

pub fn to_proto_header(header: &reth::primitives::Header) -> proto::Header {
    proto::Header {
        parent_hash: header.parent_hash.to_vec(),
        ommers_hash: header.ommers_hash.to_vec(),
        beneficiary: header.beneficiary.to_vec(),
        state_root: header.state_root.to_vec(),
        transactions_root: header.transactions_root.to_vec(),
        receipts_root: header.receipts_root.to_vec(),
        withdrawals_root: header.withdrawals_root.map(|root| root.to_vec()),
        logs_bloom: header.logs_bloom.to_vec(),
        difficulty: header.difficulty.to_le_bytes_vec(),
        number: header.number,
        gas_limit: header.gas_limit,
        gas_used: header.gas_used,
        timestamp: header.timestamp,
        mix_hash: header.mix_hash.to_vec(),
        nonce: header.nonce,
        base_fee_per_gas: header.base_fee_per_gas,
        blob_gas_used: header.blob_gas_used,
        excess_blob_gas: header.excess_blob_gas,
        parent_beacon_block_root: header.parent_beacon_block_root.map(|root| root.to_vec()),
        extra_data: header.extra_data.to_vec(),
    }
}

pub fn to_proto_transaction(
    transaction: &reth::primitives::TransactionSigned,
) -> proto::Transaction {
    let hash = transaction.hash().to_vec();
    let signature = proto::Signature {
        r: transaction.signature.r.to_le_bytes_vec(),
        s: transaction.signature.s.to_le_bytes_vec(),
        odd_y_parity: transaction.signature.odd_y_parity,
    };
    let transaction = match &transaction.transaction {
        reth::primitives::Transaction::Legacy(reth::primitives::TxLegacy {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            input,
        }) => proto::transaction::Transaction::Legacy(proto::TransactionLegacy {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_price: gas_price.to_le_bytes().to_vec(),
            gas_limit: *gas_limit,
            to: Some(to_proto_tx_kind(to)),
            value: value.to_le_bytes_vec(),
            input: input.to_vec(),
        }),
        reth::primitives::Transaction::Eip2930(reth::primitives::TxEip2930 {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            access_list,
            input,
        }) => proto::transaction::Transaction::Eip2930(proto::TransactionEip2930 {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_price: gas_price.to_le_bytes().to_vec(),
            gas_limit: *gas_limit,
            to: Some(to_proto_tx_kind(to)),
            value: value.to_le_bytes_vec(),
            access_list: access_list.iter().map(to_proto_access_list_item).collect(),
            input: input.to_vec(),
        }),
        reth::primitives::Transaction::Eip1559(reth::primitives::TxEip1559 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            input,
        }) => proto::transaction::Transaction::Eip1559(proto::TransactionEip1559 {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_limit: *gas_limit,
            max_fee_per_gas: max_fee_per_gas.to_le_bytes().to_vec(),
            max_priority_fee_per_gas: max_priority_fee_per_gas.to_le_bytes().to_vec(),
            to: Some(to_proto_tx_kind(to)),
            value: value.to_le_bytes_vec(),
            access_list: access_list.iter().map(to_proto_access_list_item).collect(),
            input: input.to_vec(),
        }),
        reth::primitives::Transaction::Eip4844(reth::primitives::TxEip4844 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            placeholder: _,
            to,
            value,
            access_list,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            input,
        }) => proto::transaction::Transaction::Eip4844(proto::TransactionEip4844 {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_limit: *gas_limit,
            max_fee_per_gas: max_fee_per_gas.to_le_bytes().to_vec(),
            max_priority_fee_per_gas: max_priority_fee_per_gas.to_le_bytes().to_vec(),
            to: to.to_vec(),
            value: value.to_le_bytes_vec(),
            access_list: access_list.iter().map(to_proto_access_list_item).collect(),
            blob_versioned_hashes: blob_versioned_hashes.iter().map(|hash| hash.to_vec()).collect(),
            max_fee_per_blob_gas: max_fee_per_blob_gas.to_le_bytes().to_vec(),
            input: input.to_vec(),
        }),
    };

    proto::Transaction { hash, signature: Some(signature), transaction: Some(transaction) }
}

pub fn to_proto_tx_kind(kind: &reth::primitives::TxKind) -> proto::TxKind {
    proto::TxKind {
        kind: match kind {
            reth::primitives::TxKind::Create => Some(proto::tx_kind::Kind::Create(())),
            reth::primitives::TxKind::Call(address) => {
                Some(proto::tx_kind::Kind::Call(address.to_vec()))
            }
        },
    }
}

pub fn to_proto_access_list_item(item: &reth::primitives::AccessListItem) -> proto::AccessListItem {
    proto::AccessListItem {
        address: item.address.to_vec(),
        storage_keys: item.storage_keys.iter().map(|key| key.to_vec()).collect(),
    }
}

pub fn to_proto_account_info(
    account_info: &reth::revm::primitives::AccountInfo,
) -> eyre::Result<proto::AccountInfo> {
    Ok(proto::AccountInfo {
        balance: account_info.balance.to_le_bytes_vec(),
        nonce: account_info.nonce,
        code_hash: account_info.code_hash.to_vec(),
        code: account_info.code.as_ref().map(to_proto_bytecode).transpose()?,
    })
}

pub fn to_proto_bytecode(
    bytecode: &reth::revm::primitives::Bytecode,
) -> eyre::Result<proto::Bytecode> {
    let bytecode = match bytecode {
        reth::revm::primitives::Bytecode::LegacyRaw(code) => {
            proto::bytecode::Bytecode::LegacyRaw(code.to_vec())
        }
        reth::revm::primitives::Bytecode::LegacyAnalyzed(legacy_analyzed) => {
            proto::bytecode::Bytecode::LegacyAnalyzed(proto::LegacyAnalyzedBytecode {
                bytecode: legacy_analyzed.bytecode().to_vec(),
                original_len: legacy_analyzed.original_len() as u64,
                jump_table: legacy_analyzed
                    .jump_table()
                    .0
                    .iter()
                    .by_vals()
                    .map(|x| x.into())
                    .collect(),
            })
        }
        reth::revm::primitives::Bytecode::Eof(_) => {
            return Err(eyre::eyre!("EOF bytecode not supported"))
        }
    };
    Ok(proto::Bytecode { bytecode: Some(bytecode) })
}

pub fn to_proto_account_status(status: reth::revm::db::AccountStatus) -> proto::AccountStatus {
    match status {
        reth::revm::db::AccountStatus::LoadedNotExisting => proto::AccountStatus::LoadedNotExisting,
        reth::revm::db::AccountStatus::Loaded => proto::AccountStatus::Loaded,
        reth::revm::db::AccountStatus::LoadedEmptyEIP161 => proto::AccountStatus::LoadedEmptyEip161,
        reth::revm::db::AccountStatus::InMemoryChange => proto::AccountStatus::InMemoryChange,
        reth::revm::db::AccountStatus::Changed => proto::AccountStatus::Changed,
        reth::revm::db::AccountStatus::Destroyed => proto::AccountStatus::Destroyed,
        reth::revm::db::AccountStatus::DestroyedChanged => proto::AccountStatus::DestroyedChanged,
        reth::revm::db::AccountStatus::DestroyedAgain => proto::AccountStatus::DestroyedAgain,
    }
}

pub fn from_proto_notification(
    notification: &proto::ExExNotification,
) -> eyre::Result<reth_exex::ExExNotification> {
    match notification.notification.as_ref().ok_or_eyre("no notification")? {
        proto::ex_ex_notification::Notification::ChainCommitted(proto::ChainCommitted { new }) => {
            Ok(reth_exex::ExExNotification::ChainCommitted {
                new: from_proto_chain(new.as_ref().ok_or_eyre("no new chain")?)?,
            })
        }
        proto::ex_ex_notification::Notification::ChainReorged(proto::ChainReorged { old, new }) => {
            Ok(reth_exex::ExExNotification::ChainReorged {
                old: from_proto_chain(old.as_ref().ok_or_eyre("no old chain")?)?,
                new: from_proto_chain(new.as_ref().ok_or_eyre("no new chain")?)?,
            })
        }
        proto::ex_ex_notification::Notification::ChainReverted(proto::ChainReverted { old }) => {
            Ok(reth_exex::ExExNotification::ChainReverted {
                old: from_proto_chain(old.as_ref().ok_or_eyre("no old chain")?)?,
            })
        }
    }
}

pub fn from_proto_chain(chain: &proto::Chain) -> eyre::Result<Arc<reth::providers::Chain>> {
    let execution_outcome = chain.execution_outcome.as_ref().ok_or_eyre("no execution outcome")?;
    let bundle = execution_outcome.bundle.as_ref().ok_or_eyre("no bundle")?;
    Ok(Arc::new(reth::providers::Chain::new(
        chain.blocks.iter().map(from_proto_block).collect::<eyre::Result<Vec<_>>>()?,
        reth::providers::ExecutionOutcome {
            bundle: reth::revm::db::BundleState {
                state: bundle
                    .state
                    .iter()
                    .map(|account| {
                        Ok((
                            Address::try_from(account.address.as_slice())?,
                            reth::revm::db::BundleAccount {
                                info: account
                                    .info
                                    .as_ref()
                                    .map(from_proto_account_info)
                                    .transpose()?,
                                original_info: account
                                    .original_info
                                    .as_ref()
                                    .map(from_proto_account_info)
                                    .transpose()?,
                                storage: account
                                    .storage
                                    .iter()
                                    .map(|slot| {
                                        Ok((
                                        U256::try_from_le_slice(slot.key.as_slice())
                                            .ok_or_eyre("failed to parse key")?,
                                        reth::revm::db::states::StorageSlot {
                                            previous_or_original_value: U256::try_from_le_slice(
                                                slot.previous_or_original_value.as_slice(),
                                            )
                                            .ok_or_eyre(
                                                "failed to parse previous or original value",
                                            )?,
                                            present_value: U256::try_from_le_slice(
                                                slot.present_value.as_slice(),
                                            )
                                            .ok_or_eyre("failed to parse present value")?,
                                        },
                                    ))
                                    })
                                    .collect::<eyre::Result<_>>()?,
                                status: from_proto_account_status(proto::AccountStatus::try_from(
                                    account.status,
                                )?),
                            },
                        ))
                    })
                    .collect::<eyre::Result<_>>()?,
                contracts: bundle
                    .contracts
                    .iter()
                    .map(|contract| {
                        Ok((
                            B256::try_from(contract.hash.as_slice())?,
                            from_proto_bytecode(
                                contract.bytecode.as_ref().ok_or_eyre("no bytecode")?,
                            )?,
                        ))
                    })
                    .collect::<eyre::Result<_>>()?,
                reverts: reth::revm::db::states::reverts::Reverts::new(
                    bundle
                        .reverts
                        .iter()
                        .map(|block_reverts| {
                            Ok(block_reverts
                                .reverts
                                .iter()
                                .map(|revert| {
                                    Ok((
                                        Address::try_from(revert.address.as_slice())?,
                                        reth::revm::db::states::reverts::AccountRevert {
                                            account: match revert.account.as_ref().ok_or_eyre("no revert account")?.revert.as_ref().ok_or_eyre("no revert account revert")? {
                                                proto::account_info_revert::Revert::DoNothing(()) => reth::revm::db::states::reverts::AccountInfoRevert::DoNothing,
                                                proto::account_info_revert::Revert::DeleteIt(()) => reth::revm::db::states::reverts::AccountInfoRevert::DeleteIt,
                                                proto::account_info_revert::Revert::RevertTo(account_info) => reth::revm::db::states::reverts::AccountInfoRevert::RevertTo(from_proto_account_info(account_info)?),
                                            },
                                            storage: revert
                                                .storage
                                                .iter()
                                                .map(|slot| Ok((
                                                    U256::try_from_le_slice(slot.key.as_slice())
                                                        .ok_or_eyre("failed to parse slot key")?,
                                                    match slot.revert.as_ref().ok_or_eyre("no slot revert")? {
                                                        proto::revert_to_slot::Revert::Some(value) => reth::revm::db::states::reverts::RevertToSlot::Some(U256::try_from_le_slice(value.as_slice()).ok_or_eyre("failed to parse slot revert")?),
                                                        proto::revert_to_slot::Revert::Destroyed(()) => reth::revm::db::states::reverts::RevertToSlot::Destroyed,
                                                    }
                                                )))
                                                .collect::<eyre::Result<_>>()?,
                                            previous_status: from_proto_account_status(
                                                proto::AccountStatus::try_from(
                                                    revert.previous_status,
                                                )?,
                                            ),
                                            wipe_storage: revert.wipe_storage,
                                        },
                                    ))
                                })
                                .collect::<eyre::Result<_>>()?)
                        })
                        .collect::<eyre::Result<_>>()?,
                ),
                state_size: bundle.state_size as usize,
                reverts_size: bundle.reverts_size as usize,
            },
            receipts: reth::primitives::Receipts::from_iter(execution_outcome
                .receipts
                .iter()
                .map(|block_receipts| {
                    Ok(block_receipts
                        .receipts
                        .iter()
                        .map(|receipt| {
                            Ok(match receipt.receipt.as_ref().ok_or_eyre("no receipt")? {
                                proto::receipt::Receipt::Empty(()) => None,
                                proto::receipt::Receipt::NonEmpty(receipt) => {
                                    Some(reth::primitives::Receipt {
                                        tx_type: match proto::TxType::try_from(receipt.tx_type)? {
                                            proto::TxType::Legacy => {
                                                reth::primitives::TxType::Legacy
                                            }
                                            proto::TxType::Eip2930 => {
                                                reth::primitives::TxType::Eip2930
                                            }
                                            proto::TxType::Eip1559 => {
                                                reth::primitives::TxType::Eip1559
                                            }
                                            proto::TxType::Eip4844 => {
                                                reth::primitives::TxType::Eip4844
                                            }
                                        },
                                        success: receipt.success,
                                        cumulative_gas_used: receipt.cumulative_gas_used,
                                        logs: receipt
                                            .logs
                                            .iter()
                                            .map(|log| {
                                                let data =
                                                    log.data.as_ref().ok_or_eyre("no log data")?;
                                                Ok(reth::primitives::Log {
                                                    address: Address::try_from(
                                                        log.address.as_slice(),
                                                    )?,
                                                    data: reth::primitives::LogData::new_unchecked(
                                                        data.topics
                                                            .iter()
                                                            .map(|topic| {
                                                                Ok(B256::try_from(
                                                                    topic.as_slice(),
                                                                )?)
                                                            })
                                                            .collect::<eyre::Result<_>>()?,
                                                        data.data.clone().into(),
                                                    ),
                                                })
                                            })
                                            .collect::<eyre::Result<_>>()?,
                                    })
                                }
                            })
                        })
                        .collect::<eyre::Result<_>>()?)
                })
                .collect::<eyre::Result<Vec<_>>>()?),
            first_block: execution_outcome.first_block,
            requests: Default::default(),
        },
        None,
    )))
}

pub fn from_proto_block(
    block: &proto::Block,
) -> eyre::Result<reth::primitives::SealedBlockWithSenders> {
    let sealed_header = block.header.as_ref().ok_or_eyre("no sealed header")?;
    let header = from_proto_header(sealed_header.header.as_ref().ok_or_eyre("no header")?)?;
    let sealed_header = reth::primitives::SealedHeader::new(
        header,
        BlockHash::try_from(sealed_header.hash.as_slice())?,
    );

    let transactions =
        block.body.iter().map(from_proto_transaction).collect::<eyre::Result<_>>()?;
    let ommers = block.ommers.iter().map(from_proto_header).collect::<eyre::Result<_>>()?;
    let senders = block
        .senders
        .iter()
        .map(|sender| Address::try_from(sender.as_slice()))
        .collect::<Result<_, _>>()?;

    reth::primitives::SealedBlockWithSenders::new(
        reth::primitives::SealedBlock::new(
            sealed_header,
            reth::primitives::BlockBody {
                transactions,
                ommers,
                withdrawals: Default::default(),
                requests: Default::default(),
            },
        ),
        senders,
    )
    .ok_or_eyre("senders do not match transactions")
}

pub fn from_proto_header(header: &proto::Header) -> eyre::Result<reth::primitives::Header> {
    Ok(reth::primitives::Header {
        parent_hash: B256::try_from(header.parent_hash.as_slice())?,
        ommers_hash: B256::try_from(header.ommers_hash.as_slice())?,
        beneficiary: Address::try_from(header.beneficiary.as_slice())?,
        state_root: B256::try_from(header.state_root.as_slice())?,
        transactions_root: B256::try_from(header.transactions_root.as_slice())?,
        receipts_root: B256::try_from(header.receipts_root.as_slice())?,
        withdrawals_root: header
            .withdrawals_root
            .as_ref()
            .map(|root| B256::try_from(root.as_slice()))
            .transpose()?,
        logs_bloom: Bloom::try_from(header.logs_bloom.as_slice())?,
        difficulty: U256::try_from_le_slice(&header.difficulty)
            .ok_or_eyre("failed to parse difficulty")?,
        number: header.number,
        gas_limit: header.gas_limit,
        gas_used: header.gas_used,
        timestamp: header.timestamp,
        mix_hash: B256::try_from(header.mix_hash.as_slice())?,
        nonce: header.nonce,
        base_fee_per_gas: header.base_fee_per_gas,
        blob_gas_used: header.blob_gas_used,
        excess_blob_gas: header.excess_blob_gas,
        parent_beacon_block_root: header
            .parent_beacon_block_root
            .as_ref()
            .map(|root| B256::try_from(root.as_slice()))
            .transpose()?,
        requests_root: None,
        extra_data: header.extra_data.as_slice().to_vec().into(),
    })
}

pub fn from_proto_transaction(
    transaction: &proto::Transaction,
) -> eyre::Result<reth::primitives::TransactionSigned> {
    let hash = TxHash::try_from(transaction.hash.as_slice())?;
    let signature = transaction.signature.as_ref().ok_or_eyre("no signature")?;
    let signature = reth::primitives::Signature {
        r: U256::try_from_le_slice(signature.r.as_slice()).ok_or_eyre("failed to parse r")?,
        s: U256::try_from_le_slice(signature.s.as_slice()).ok_or_eyre("failed to parse s")?,
        odd_y_parity: signature.odd_y_parity,
    };
    let transaction = match transaction.transaction.as_ref().ok_or_eyre("no transaction")? {
        proto::transaction::Transaction::Legacy(proto::TransactionLegacy {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            input,
        }) => reth::primitives::Transaction::Legacy(reth::primitives::TxLegacy {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_price: u128::from_le_bytes(gas_price.as_slice().try_into()?),
            gas_limit: *gas_limit,
            to: from_proto_tx_kind(to.as_ref().ok_or_eyre("no to")?)?,
            value: U256::try_from_le_slice(value.as_slice()).ok_or_eyre("failed to parse value")?,
            input: input.to_vec().into(),
        }),
        proto::transaction::Transaction::Eip2930(proto::TransactionEip2930 {
            chain_id,
            nonce,
            gas_price,
            gas_limit,
            to,
            value,
            access_list,
            input,
        }) => reth::primitives::Transaction::Eip2930(reth::primitives::TxEip2930 {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_price: u128::from_le_bytes(gas_price.as_slice().try_into()?),
            gas_limit: *gas_limit,
            to: from_proto_tx_kind(to.as_ref().ok_or_eyre("no to")?)?,
            value: U256::try_from_le_slice(value.as_slice()).ok_or_eyre("failed to parse value")?,
            access_list: access_list
                .iter()
                .map(from_proto_access_list_item)
                .collect::<eyre::Result<Vec<_>>>()?
                .into(),
            input: input.to_vec().into(),
        }),
        proto::transaction::Transaction::Eip1559(proto::TransactionEip1559 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            input,
        }) => reth::primitives::Transaction::Eip1559(reth::primitives::TxEip1559 {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_limit: *gas_limit,
            max_fee_per_gas: u128::from_le_bytes(max_fee_per_gas.as_slice().try_into()?),
            max_priority_fee_per_gas: u128::from_le_bytes(
                max_priority_fee_per_gas.as_slice().try_into()?,
            ),
            to: from_proto_tx_kind(to.as_ref().ok_or_eyre("no to")?)?,
            value: U256::try_from_le_slice(value.as_slice()).ok_or_eyre("failed to parse value")?,
            access_list: access_list
                .iter()
                .map(from_proto_access_list_item)
                .collect::<eyre::Result<Vec<_>>>()?
                .into(),
            input: input.to_vec().into(),
        }),
        proto::transaction::Transaction::Eip4844(proto::TransactionEip4844 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to,
            value,
            access_list,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            input,
        }) => reth::primitives::Transaction::Eip4844(reth::primitives::TxEip4844 {
            chain_id: chain_id.clone(),
            nonce: *nonce,
            gas_limit: *gas_limit,
            max_fee_per_gas: u128::from_le_bytes(max_fee_per_gas.as_slice().try_into()?),
            max_priority_fee_per_gas: u128::from_le_bytes(
                max_priority_fee_per_gas.as_slice().try_into()?,
            ),
            placeholder: None,
            to: Address::try_from(to.as_slice())?,
            value: U256::try_from_le_slice(value.as_slice()).ok_or_eyre("failed to parse value")?,
            access_list: access_list
                .iter()
                .map(from_proto_access_list_item)
                .collect::<eyre::Result<Vec<_>>>()?
                .into(),
            blob_versioned_hashes: blob_versioned_hashes
                .iter()
                .map(|hash| B256::try_from(hash.as_slice()))
                .collect::<Result<_, _>>()?,
            max_fee_per_blob_gas: u128::from_le_bytes(max_fee_per_blob_gas.as_slice().try_into()?),
            input: input.to_vec().into(),
        }),
    };

    Ok(reth::primitives::TransactionSigned { hash, signature, transaction })
}

pub fn from_proto_tx_kind(tx_kind: &proto::TxKind) -> eyre::Result<reth::primitives::TxKind> {
    Ok(match tx_kind.kind.as_ref().ok_or_eyre("no kind")? {
        proto::tx_kind::Kind::Create(()) => reth::primitives::TxKind::Create,
        proto::tx_kind::Kind::Call(address) => {
            reth::primitives::TxKind::Call(Address::try_from(address.as_slice())?)
        }
    })
}

pub fn from_proto_access_list_item(
    item: &proto::AccessListItem,
) -> eyre::Result<reth::primitives::AccessListItem> {
    Ok(reth::primitives::AccessListItem {
        address: Address::try_from(item.address.as_slice())?,
        storage_keys: item
            .storage_keys
            .iter()
            .map(|key| B256::try_from(key.as_slice()))
            .collect::<Result<_, _>>()?,
    })
}

pub fn from_proto_account_info(
    account_info: &proto::AccountInfo,
) -> eyre::Result<reth::revm::primitives::AccountInfo> {
    Ok(reth::revm::primitives::AccountInfo {
        balance: U256::try_from_le_slice(account_info.balance.as_slice())
            .ok_or_eyre("failed to parse balance")?,
        nonce: account_info.nonce,
        code_hash: B256::try_from(account_info.code_hash.as_slice())?,
        code: account_info.code.as_ref().map(from_proto_bytecode).transpose()?,
    })
}

pub fn from_proto_bytecode(
    bytecode: &proto::Bytecode,
) -> eyre::Result<reth::revm::primitives::Bytecode> {
    let bytecode = match bytecode.bytecode.as_ref().ok_or_eyre("no bytecode")? {
        proto::bytecode::Bytecode::LegacyRaw(code) => {
            reth::revm::primitives::Bytecode::LegacyRaw(code.clone().into())
        }
        proto::bytecode::Bytecode::LegacyAnalyzed(legacy_analyzed) => {
            reth::revm::primitives::Bytecode::LegacyAnalyzed(
                reth::revm::primitives::LegacyAnalyzedBytecode::new(
                    legacy_analyzed.bytecode.clone().into(),
                    legacy_analyzed.original_len as usize,
                    reth::revm::primitives::JumpTable::from_slice(
                        legacy_analyzed
                            .jump_table
                            .iter()
                            .map(|dest| *dest as u8)
                            .collect::<Vec<_>>()
                            .as_slice(),
                    ),
                ),
            )
        }
    };
    Ok(bytecode)
}

pub fn from_proto_account_status(status: proto::AccountStatus) -> reth::revm::db::AccountStatus {
    match status {
        proto::AccountStatus::LoadedNotExisting => reth::revm::db::AccountStatus::LoadedNotExisting,
        proto::AccountStatus::Loaded => reth::revm::db::AccountStatus::Loaded,
        proto::AccountStatus::LoadedEmptyEip161 => reth::revm::db::AccountStatus::LoadedEmptyEIP161,
        proto::AccountStatus::InMemoryChange => reth::revm::db::AccountStatus::InMemoryChange,
        proto::AccountStatus::Changed => reth::revm::db::AccountStatus::Changed,
        proto::AccountStatus::Destroyed => reth::revm::db::AccountStatus::Destroyed,
        proto::AccountStatus::DestroyedChanged => reth::revm::db::AccountStatus::DestroyedChanged,
        proto::AccountStatus::DestroyedAgain => reth::revm::db::AccountStatus::DestroyedAgain,
    }
}
