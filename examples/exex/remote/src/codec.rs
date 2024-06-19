use std::sync::Arc;

use eyre::OptionExt;
use reth::primitives::{Address, BlockHash, Bloom, TxHash, B256, U256};

use crate::proto;

impl TryFrom<&reth_exex::ExExNotification> for proto::ExExNotification {
    type Error = eyre::Error;

    fn try_from(notification: &reth_exex::ExExNotification) -> Result<Self, Self::Error> {
        let notification = match notification {
            reth_exex::ExExNotification::ChainCommitted { new } => {
                proto::ex_ex_notification::Notification::ChainCommitted(proto::ChainCommitted {
                    new: Some(new.as_ref().try_into()?),
                })
            }
            reth_exex::ExExNotification::ChainReorged { old, new } => {
                proto::ex_ex_notification::Notification::ChainReorged(proto::ChainReorged {
                    old: Some(old.as_ref().try_into()?),
                    new: Some(new.as_ref().try_into()?),
                })
            }
            reth_exex::ExExNotification::ChainReverted { old } => {
                proto::ex_ex_notification::Notification::ChainReverted(proto::ChainReverted {
                    old: Some(old.as_ref().try_into()?),
                })
            }
        };

        Ok(proto::ExExNotification { notification: Some(notification) })
    }
}

impl TryFrom<&reth::providers::Chain> for proto::Chain {
    type Error = eyre::Error;

    fn try_from(chain: &reth::providers::Chain) -> Result<Self, Self::Error> {
        let bundle_state = chain.execution_outcome().state();
        Ok(proto::Chain {
            blocks: chain
                .blocks_iter()
                .map(|block| {
                    Ok(proto::Block {
                        header: Some(proto::SealedHeader {
                            hash: block.header.hash().to_vec(),
                            header: Some(block.header.header().into()),
                        }),
                        body: block
                            .transactions()
                            .map(TryInto::try_into)
                            .collect::<eyre::Result<_>>()?,
                        ommers: block.ommers.iter().map(Into::into).collect(),
                        senders: block.senders.iter().map(|sender| sender.to_vec()).collect(),
                    })
                })
                .collect::<eyre::Result<_>>()?,
            execution_outcome: Some(proto::ExecutionOutcome {
                bundle: Some(proto::BundleState {
                    state: bundle_state
                        .state
                        .iter()
                        .map(|(address, account)| (*address, account).try_into())
                        .collect::<eyre::Result<_>>()?,
                    contracts: bundle_state
                        .contracts
                        .iter()
                        .map(|(hash, bytecode)| {
                            Ok(proto::ContractBytecode {
                                hash: hash.to_vec(),
                                bytecode: Some(bytecode.try_into()?),
                            })
                        })
                        .collect::<eyre::Result<_>>()?,
                    reverts: bundle_state
                        .reverts
                        .iter()
                        .map(|block_reverts| {
                            Ok(proto::BlockReverts {
                                reverts: block_reverts
                                    .iter()
                                    .map(|(address, revert)| (*address, revert).try_into())
                                    .collect::<eyre::Result<_>>()?,
                            })
                        })
                        .collect::<eyre::Result<_>>()?,
                    state_size: bundle_state.state_size as u64,
                    reverts_size: bundle_state.reverts_size as u64,
                }),
                receipts: chain
                    .execution_outcome()
                    .receipts()
                    .iter()
                    .map(|block_receipts| {
                        Ok(proto::BlockReceipts {
                            receipts: block_receipts
                                .iter()
                                .map(TryInto::try_into)
                                .collect::<eyre::Result<_>>()?,
                        })
                    })
                    .collect::<eyre::Result<_>>()?,
                first_block: chain.execution_outcome().first_block,
            }),
        })
    }
}

impl From<&reth::primitives::Header> for proto::Header {
    fn from(header: &reth::primitives::Header) -> Self {
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
}

impl TryFrom<&reth::primitives::TransactionSigned> for proto::Transaction {
    type Error = eyre::Error;

    fn try_from(transaction: &reth::primitives::TransactionSigned) -> Result<Self, Self::Error> {
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_price: gas_price.to_le_bytes().to_vec(),
                gas_limit: *gas_limit,
                to: Some(to.into()),
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_price: gas_price.to_le_bytes().to_vec(),
                gas_limit: *gas_limit,
                to: Some(to.into()),
                value: value.to_le_bytes_vec(),
                access_list: access_list.iter().map(Into::into).collect(),
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_limit: *gas_limit,
                max_fee_per_gas: max_fee_per_gas.to_le_bytes().to_vec(),
                max_priority_fee_per_gas: max_priority_fee_per_gas.to_le_bytes().to_vec(),
                to: Some(to.into()),
                value: value.to_le_bytes_vec(),
                access_list: access_list.iter().map(Into::into).collect(),
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_limit: *gas_limit,
                max_fee_per_gas: max_fee_per_gas.to_le_bytes().to_vec(),
                max_priority_fee_per_gas: max_priority_fee_per_gas.to_le_bytes().to_vec(),
                to: to.to_vec(),
                value: value.to_le_bytes_vec(),
                access_list: access_list.iter().map(Into::into).collect(),
                blob_versioned_hashes: blob_versioned_hashes
                    .iter()
                    .map(|hash| hash.to_vec())
                    .collect(),
                max_fee_per_blob_gas: max_fee_per_blob_gas.to_le_bytes().to_vec(),
                input: input.to_vec(),
            }),
            #[cfg(feature = "optimism")]
            reth::primitives::Transaction::Deposit(_) => {
                eyre::bail!("deposit transaction not supported")
            }
        };

        Ok(proto::Transaction { hash, signature: Some(signature), transaction: Some(transaction) })
    }
}

impl From<&reth::primitives::TxKind> for proto::TxKind {
    fn from(kind: &reth::primitives::TxKind) -> Self {
        proto::TxKind {
            kind: match kind {
                reth::primitives::TxKind::Create => Some(proto::tx_kind::Kind::Create(())),
                reth::primitives::TxKind::Call(address) => {
                    Some(proto::tx_kind::Kind::Call(address.to_vec()))
                }
            },
        }
    }
}

impl From<&reth::primitives::AccessListItem> for proto::AccessListItem {
    fn from(item: &reth::primitives::AccessListItem) -> Self {
        proto::AccessListItem {
            address: item.address.to_vec(),
            storage_keys: item.storage_keys.iter().map(|key| key.to_vec()).collect(),
        }
    }
}

impl TryFrom<(Address, &reth::revm::db::BundleAccount)> for proto::BundleAccount {
    type Error = eyre::Error;

    fn try_from(
        (address, account): (Address, &reth::revm::db::BundleAccount),
    ) -> Result<Self, Self::Error> {
        Ok(proto::BundleAccount {
            address: address.to_vec(),
            info: account.info.as_ref().map(TryInto::try_into).transpose()?,
            original_info: account.original_info.as_ref().map(TryInto::try_into).transpose()?,
            storage: account
                .storage
                .iter()
                .map(|(key, slot)| proto::StorageSlot {
                    key: key.to_le_bytes_vec(),
                    previous_or_original_value: slot.previous_or_original_value.to_le_bytes_vec(),
                    present_value: slot.present_value.to_le_bytes_vec(),
                })
                .collect(),
            status: proto::AccountStatus::from(account.status) as i32,
        })
    }
}

impl TryFrom<&reth::revm::primitives::AccountInfo> for proto::AccountInfo {
    type Error = eyre::Error;

    fn try_from(account_info: &reth::revm::primitives::AccountInfo) -> Result<Self, Self::Error> {
        Ok(proto::AccountInfo {
            balance: account_info.balance.to_le_bytes_vec(),
            nonce: account_info.nonce,
            code_hash: account_info.code_hash.to_vec(),
            code: account_info.code.as_ref().map(TryInto::try_into).transpose()?,
        })
    }
}

impl TryFrom<&reth::revm::primitives::Bytecode> for proto::Bytecode {
    type Error = eyre::Error;

    fn try_from(bytecode: &reth::revm::primitives::Bytecode) -> Result<Self, Self::Error> {
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
                eyre::bail!("EOF bytecode not supported");
            }
        };
        Ok(proto::Bytecode { bytecode: Some(bytecode) })
    }
}

impl From<reth::revm::db::AccountStatus> for proto::AccountStatus {
    fn from(status: reth::revm::db::AccountStatus) -> Self {
        match status {
            reth::revm::db::AccountStatus::LoadedNotExisting => {
                proto::AccountStatus::LoadedNotExisting
            }
            reth::revm::db::AccountStatus::Loaded => proto::AccountStatus::Loaded,
            reth::revm::db::AccountStatus::LoadedEmptyEIP161 => {
                proto::AccountStatus::LoadedEmptyEip161
            }
            reth::revm::db::AccountStatus::InMemoryChange => proto::AccountStatus::InMemoryChange,
            reth::revm::db::AccountStatus::Changed => proto::AccountStatus::Changed,
            reth::revm::db::AccountStatus::Destroyed => proto::AccountStatus::Destroyed,
            reth::revm::db::AccountStatus::DestroyedChanged => {
                proto::AccountStatus::DestroyedChanged
            }
            reth::revm::db::AccountStatus::DestroyedAgain => proto::AccountStatus::DestroyedAgain,
        }
    }
}

impl TryFrom<(Address, &reth::revm::db::states::reverts::AccountRevert)> for proto::Revert {
    type Error = eyre::Error;

    fn try_from(
        (address, revert): (Address, &reth::revm::db::states::reverts::AccountRevert),
    ) -> Result<Self, Self::Error> {
        Ok(proto::Revert {
            address: address.to_vec(),
            account: Some(proto::AccountInfoRevert {
                revert: Some(match &revert.account {
                    reth::revm::db::states::reverts::AccountInfoRevert::DoNothing => {
                        proto::account_info_revert::Revert::DoNothing(())
                    }
                    reth::revm::db::states::reverts::AccountInfoRevert::DeleteIt => {
                        proto::account_info_revert::Revert::DeleteIt(())
                    }
                    reth::revm::db::states::reverts::AccountInfoRevert::RevertTo(account_info) => {
                        proto::account_info_revert::Revert::RevertTo(account_info.try_into()?)
                    }
                }),
            }),
            storage: revert
                .storage
                .iter()
                .map(|(key, slot)| {
                    Ok(proto::RevertToSlot {
                        key: key.to_le_bytes_vec(),
                        revert: Some(match slot {
                            reth::revm::db::RevertToSlot::Some(value) => {
                                proto::revert_to_slot::Revert::Some(value.to_le_bytes_vec())
                            }
                            reth::revm::db::RevertToSlot::Destroyed => {
                                proto::revert_to_slot::Revert::Destroyed(())
                            }
                        }),
                    })
                })
                .collect::<eyre::Result<_>>()?,
            previous_status: proto::AccountStatus::from(revert.previous_status) as i32,
            wipe_storage: revert.wipe_storage,
        })
    }
}

impl TryFrom<&Option<reth::primitives::Receipt>> for proto::Receipt {
    type Error = eyre::Error;

    fn try_from(receipt: &Option<reth::primitives::Receipt>) -> Result<Self, Self::Error> {
        Ok(proto::Receipt {
            receipt: Some(
                receipt
                    .as_ref()
                    .map_or(eyre::Ok(proto::receipt::Receipt::Empty(())), |receipt| {
                        Ok(proto::receipt::Receipt::NonEmpty(receipt.try_into()?))
                    })?,
            ),
        })
    }
}

impl TryFrom<&reth::primitives::Receipt> for proto::NonEmptyReceipt {
    type Error = eyre::Error;

    fn try_from(receipt: &reth::primitives::Receipt) -> Result<Self, Self::Error> {
        Ok(proto::NonEmptyReceipt {
            tx_type: match receipt.tx_type {
                reth::primitives::TxType::Legacy => proto::TxType::Legacy,
                reth::primitives::TxType::Eip2930 => proto::TxType::Eip2930,
                reth::primitives::TxType::Eip1559 => proto::TxType::Eip1559,
                reth::primitives::TxType::Eip4844 => proto::TxType::Eip4844,
                #[cfg(feature = "optimism")]
                reth::primitives::TxType::Deposit => {
                    eyre::bail!("deposit transaction not supported")
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
                        topics: log.data.topics().iter().map(|topic| topic.to_vec()).collect(),
                        data: log.data.data.to_vec(),
                    }),
                })
                .collect(),
        })
    }
}

impl TryFrom<&proto::ExExNotification> for reth_exex::ExExNotification {
    type Error = eyre::Error;

    fn try_from(notification: &proto::ExExNotification) -> Result<Self, Self::Error> {
        Ok(match notification.notification.as_ref().ok_or_eyre("no notification")? {
            proto::ex_ex_notification::Notification::ChainCommitted(proto::ChainCommitted {
                new,
            }) => reth_exex::ExExNotification::ChainCommitted {
                new: Arc::new(new.as_ref().ok_or_eyre("no new chain")?.try_into()?),
            },
            proto::ex_ex_notification::Notification::ChainReorged(proto::ChainReorged {
                old,
                new,
            }) => reth_exex::ExExNotification::ChainReorged {
                old: Arc::new(old.as_ref().ok_or_eyre("no old chain")?.try_into()?),
                new: Arc::new(new.as_ref().ok_or_eyre("no new chain")?.try_into()?),
            },
            proto::ex_ex_notification::Notification::ChainReverted(proto::ChainReverted {
                old,
            }) => reth_exex::ExExNotification::ChainReverted {
                old: Arc::new(old.as_ref().ok_or_eyre("no old chain")?.try_into()?),
            },
        })
    }
}

impl TryFrom<&proto::Chain> for reth::providers::Chain {
    type Error = eyre::Error;

    fn try_from(chain: &proto::Chain) -> Result<Self, Self::Error> {
        let execution_outcome =
            chain.execution_outcome.as_ref().ok_or_eyre("no execution outcome")?;
        let bundle = execution_outcome.bundle.as_ref().ok_or_eyre("no bundle")?;
        Ok(reth::providers::Chain::new(
            chain.blocks.iter().map(TryInto::try_into).collect::<eyre::Result<Vec<_>>>()?,
            reth::providers::ExecutionOutcome {
                bundle: reth::revm::db::BundleState {
                    state: bundle
                        .state
                        .iter()
                        .map(TryInto::try_into)
                        .collect::<eyre::Result<_>>()?,
                    contracts: bundle
                        .contracts
                        .iter()
                        .map(|contract| {
                            Ok((
                                B256::try_from(contract.hash.as_slice())?,
                                contract.bytecode.as_ref().ok_or_eyre("no bytecode")?.try_into()?,
                            ))
                        })
                        .collect::<eyre::Result<_>>()?,
                    reverts: reth::revm::db::states::reverts::Reverts::new(
                        bundle
                            .reverts
                            .iter()
                            .map(|block_reverts| {
                                block_reverts
                                    .reverts
                                    .iter()
                                    .map(TryInto::try_into)
                                    .collect::<eyre::Result<_>>()
                            })
                            .collect::<eyre::Result<_>>()?,
                    ),
                    state_size: bundle.state_size as usize,
                    reverts_size: bundle.reverts_size as usize,
                },
                receipts: reth::primitives::Receipts::from_iter(
                    execution_outcome
                        .receipts
                        .iter()
                        .map(|block_receipts| {
                            block_receipts
                                .receipts
                                .iter()
                                .map(TryInto::try_into)
                                .collect::<eyre::Result<_>>()
                        })
                        .collect::<eyre::Result<Vec<_>>>()?,
                ),
                first_block: execution_outcome.first_block,
                requests: Default::default(),
            },
            None,
        ))
    }
}

impl TryFrom<&proto::Block> for reth::primitives::SealedBlockWithSenders {
    type Error = eyre::Error;

    fn try_from(block: &proto::Block) -> Result<Self, Self::Error> {
        let sealed_header = block.header.as_ref().ok_or_eyre("no sealed header")?;
        let header = sealed_header.header.as_ref().ok_or_eyre("no header")?.try_into()?;
        let sealed_header = reth::primitives::SealedHeader::new(
            header,
            BlockHash::try_from(sealed_header.hash.as_slice())?,
        );

        let transactions = block.body.iter().map(TryInto::try_into).collect::<eyre::Result<_>>()?;
        let ommers = block.ommers.iter().map(TryInto::try_into).collect::<eyre::Result<_>>()?;
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
}

impl TryFrom<&proto::Header> for reth::primitives::Header {
    type Error = eyre::Error;

    fn try_from(header: &proto::Header) -> Result<Self, Self::Error> {
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
}

impl TryFrom<&proto::Transaction> for reth::primitives::TransactionSigned {
    type Error = eyre::Error;

    fn try_from(transaction: &proto::Transaction) -> Result<Self, Self::Error> {
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_price: u128::from_le_bytes(gas_price.as_slice().try_into()?),
                gas_limit: *gas_limit,
                to: to.as_ref().ok_or_eyre("no to")?.try_into()?,
                value: U256::try_from_le_slice(value.as_slice())
                    .ok_or_eyre("failed to parse value")?,
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_price: u128::from_le_bytes(gas_price.as_slice().try_into()?),
                gas_limit: *gas_limit,
                to: to.as_ref().ok_or_eyre("no to")?.try_into()?,
                value: U256::try_from_le_slice(value.as_slice())
                    .ok_or_eyre("failed to parse value")?,
                access_list: access_list
                    .iter()
                    .map(TryInto::try_into)
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_limit: *gas_limit,
                max_fee_per_gas: u128::from_le_bytes(max_fee_per_gas.as_slice().try_into()?),
                max_priority_fee_per_gas: u128::from_le_bytes(
                    max_priority_fee_per_gas.as_slice().try_into()?,
                ),
                to: to.as_ref().ok_or_eyre("no to")?.try_into()?,
                value: U256::try_from_le_slice(value.as_slice())
                    .ok_or_eyre("failed to parse value")?,
                access_list: access_list
                    .iter()
                    .map(TryInto::try_into)
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
                chain_id: *chain_id,
                nonce: *nonce,
                gas_limit: *gas_limit,
                max_fee_per_gas: u128::from_le_bytes(max_fee_per_gas.as_slice().try_into()?),
                max_priority_fee_per_gas: u128::from_le_bytes(
                    max_priority_fee_per_gas.as_slice().try_into()?,
                ),
                placeholder: None,
                to: Address::try_from(to.as_slice())?,
                value: U256::try_from_le_slice(value.as_slice())
                    .ok_or_eyre("failed to parse value")?,
                access_list: access_list
                    .iter()
                    .map(TryInto::try_into)
                    .collect::<eyre::Result<Vec<_>>>()?
                    .into(),
                blob_versioned_hashes: blob_versioned_hashes
                    .iter()
                    .map(|hash| B256::try_from(hash.as_slice()))
                    .collect::<Result<_, _>>()?,
                max_fee_per_blob_gas: u128::from_le_bytes(
                    max_fee_per_blob_gas.as_slice().try_into()?,
                ),
                input: input.to_vec().into(),
            }),
        };

        Ok(reth::primitives::TransactionSigned { hash, signature, transaction })
    }
}

impl TryFrom<&proto::TxKind> for reth::primitives::TxKind {
    type Error = eyre::Error;

    fn try_from(tx_kind: &proto::TxKind) -> Result<Self, Self::Error> {
        Ok(match tx_kind.kind.as_ref().ok_or_eyre("no kind")? {
            proto::tx_kind::Kind::Create(()) => reth::primitives::TxKind::Create,
            proto::tx_kind::Kind::Call(address) => {
                reth::primitives::TxKind::Call(Address::try_from(address.as_slice())?)
            }
        })
    }
}

impl TryFrom<&proto::AccessListItem> for reth::primitives::AccessListItem {
    type Error = eyre::Error;

    fn try_from(item: &proto::AccessListItem) -> Result<Self, Self::Error> {
        Ok(reth::primitives::AccessListItem {
            address: Address::try_from(item.address.as_slice())?,
            storage_keys: item
                .storage_keys
                .iter()
                .map(|key| B256::try_from(key.as_slice()))
                .collect::<Result<_, _>>()?,
        })
    }
}

impl TryFrom<&proto::AccountInfo> for reth::revm::primitives::AccountInfo {
    type Error = eyre::Error;

    fn try_from(account_info: &proto::AccountInfo) -> Result<Self, Self::Error> {
        Ok(reth::revm::primitives::AccountInfo {
            balance: U256::try_from_le_slice(account_info.balance.as_slice())
                .ok_or_eyre("failed to parse balance")?,
            nonce: account_info.nonce,
            code_hash: B256::try_from(account_info.code_hash.as_slice())?,
            code: account_info.code.as_ref().map(TryInto::try_into).transpose()?,
        })
    }
}

impl TryFrom<&proto::Bytecode> for reth::revm::primitives::Bytecode {
    type Error = eyre::Error;

    fn try_from(bytecode: &proto::Bytecode) -> Result<Self, Self::Error> {
        Ok(match bytecode.bytecode.as_ref().ok_or_eyre("no bytecode")? {
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
        })
    }
}

impl From<proto::AccountStatus> for reth::revm::db::AccountStatus {
    fn from(status: proto::AccountStatus) -> Self {
        match status {
            proto::AccountStatus::LoadedNotExisting => {
                reth::revm::db::AccountStatus::LoadedNotExisting
            }
            proto::AccountStatus::Loaded => reth::revm::db::AccountStatus::Loaded,
            proto::AccountStatus::LoadedEmptyEip161 => {
                reth::revm::db::AccountStatus::LoadedEmptyEIP161
            }
            proto::AccountStatus::InMemoryChange => reth::revm::db::AccountStatus::InMemoryChange,
            proto::AccountStatus::Changed => reth::revm::db::AccountStatus::Changed,
            proto::AccountStatus::Destroyed => reth::revm::db::AccountStatus::Destroyed,
            proto::AccountStatus::DestroyedChanged => {
                reth::revm::db::AccountStatus::DestroyedChanged
            }
            proto::AccountStatus::DestroyedAgain => reth::revm::db::AccountStatus::DestroyedAgain,
        }
    }
}

impl TryFrom<&proto::BundleAccount> for (Address, reth::revm::db::BundleAccount) {
    type Error = eyre::Error;

    fn try_from(account: &proto::BundleAccount) -> Result<Self, Self::Error> {
        Ok((
            Address::try_from(account.address.as_slice())?,
            reth::revm::db::BundleAccount {
                info: account.info.as_ref().map(TryInto::try_into).transpose()?,
                original_info: account.original_info.as_ref().map(TryInto::try_into).transpose()?,
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
                                .ok_or_eyre("failed to parse previous or original value")?,
                                present_value: U256::try_from_le_slice(
                                    slot.present_value.as_slice(),
                                )
                                .ok_or_eyre("failed to parse present value")?,
                            },
                        ))
                    })
                    .collect::<eyre::Result<_>>()?,
                status: proto::AccountStatus::try_from(account.status)?.into(),
            },
        ))
    }
}

impl TryFrom<&proto::Revert> for (Address, reth::revm::db::states::reverts::AccountRevert) {
    type Error = eyre::Error;

    fn try_from(revert: &proto::Revert) -> Result<Self, Self::Error> {
        Ok((
            Address::try_from(revert.address.as_slice())?,
            reth::revm::db::states::reverts::AccountRevert {
                account: match revert
                    .account
                    .as_ref()
                    .ok_or_eyre("no revert account")?
                    .revert
                    .as_ref()
                    .ok_or_eyre("no revert account revert")?
                {
                    proto::account_info_revert::Revert::DoNothing(()) => {
                        reth::revm::db::states::reverts::AccountInfoRevert::DoNothing
                    }
                    proto::account_info_revert::Revert::DeleteIt(()) => {
                        reth::revm::db::states::reverts::AccountInfoRevert::DeleteIt
                    }
                    proto::account_info_revert::Revert::RevertTo(account_info) => {
                        reth::revm::db::states::reverts::AccountInfoRevert::RevertTo(
                            account_info.try_into()?,
                        )
                    }
                },
                storage: revert
                    .storage
                    .iter()
                    .map(|slot| {
                        Ok((
                            U256::try_from_le_slice(slot.key.as_slice())
                                .ok_or_eyre("failed to parse slot key")?,
                            match slot.revert.as_ref().ok_or_eyre("no slot revert")? {
                                proto::revert_to_slot::Revert::Some(value) => {
                                    reth::revm::db::states::reverts::RevertToSlot::Some(
                                        U256::try_from_le_slice(value.as_slice())
                                            .ok_or_eyre("failed to parse slot revert")?,
                                    )
                                }
                                proto::revert_to_slot::Revert::Destroyed(()) => {
                                    reth::revm::db::states::reverts::RevertToSlot::Destroyed
                                }
                            },
                        ))
                    })
                    .collect::<eyre::Result<_>>()?,
                previous_status: proto::AccountStatus::try_from(revert.previous_status)?.into(),
                wipe_storage: revert.wipe_storage,
            },
        ))
    }
}

impl TryFrom<&proto::Receipt> for Option<reth::primitives::Receipt> {
    type Error = eyre::Error;

    fn try_from(receipt: &proto::Receipt) -> Result<Self, Self::Error> {
        Ok(match receipt.receipt.as_ref().ok_or_eyre("no receipt")? {
            proto::receipt::Receipt::Empty(()) => None,
            proto::receipt::Receipt::NonEmpty(receipt) => Some(receipt.try_into()?),
        })
    }
}

impl TryFrom<&proto::NonEmptyReceipt> for reth::primitives::Receipt {
    type Error = eyre::Error;

    fn try_from(receipt: &proto::NonEmptyReceipt) -> Result<Self, Self::Error> {
        Ok(reth::primitives::Receipt {
            tx_type: match proto::TxType::try_from(receipt.tx_type)? {
                proto::TxType::Legacy => reth::primitives::TxType::Legacy,
                proto::TxType::Eip2930 => reth::primitives::TxType::Eip2930,
                proto::TxType::Eip1559 => reth::primitives::TxType::Eip1559,
                proto::TxType::Eip4844 => reth::primitives::TxType::Eip4844,
            },
            success: receipt.success,
            cumulative_gas_used: receipt.cumulative_gas_used,
            logs: receipt
                .logs
                .iter()
                .map(|log| {
                    let data = log.data.as_ref().ok_or_eyre("no log data")?;
                    Ok(reth::primitives::Log {
                        address: Address::try_from(log.address.as_slice())?,
                        data: reth::primitives::LogData::new_unchecked(
                            data.topics
                                .iter()
                                .map(|topic| Ok(B256::try_from(topic.as_slice())?))
                                .collect::<eyre::Result<_>>()?,
                            data.data.clone().into(),
                        ),
                    })
                })
                .collect::<eyre::Result<_>>()?,
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        })
    }
}
