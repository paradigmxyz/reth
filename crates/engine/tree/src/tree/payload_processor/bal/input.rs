use alloy_eip7928::{bal::RawBal, BlockAccessIndex, BlockAccessListGasError};
use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Error as RlpError, Header};
use reth_consensus::ConsensusError;
use revm_state::{
    bal::{AccountBal, AccountInfoBal, Bal as RevmBal, BalWrites, StorageBal},
    primitives::{Address, AddressIndexMap, B256, U256},
    Bytecode,
};
use std::sync::Arc;

/// Received block access list decoded into the form used by BAL execution.
#[derive(Debug)]
pub struct ReceivedBal {
    raw: RawBal,
    revm: Arc<RevmBal>,
}

impl ReceivedBal {
    /// Decodes a received raw EIP-7928 BAL directly into revm's representation.
    pub fn from_rlp_bytes(raw: Bytes) -> Result<Self, ConsensusError> {
        let raw = RawBal::new(raw);
        let mut slice = raw.as_raw().as_ref();
        let revm = decode_revm_bal(&mut slice).map_err(|err| {
            ConsensusError::BlockAccessListInvalid(format!("failed to decode BAL: {err}"))
        })?;
        if !slice.is_empty() {
            return Err(ConsensusError::BlockAccessListInvalid(
                "failed to decode BAL: trailing bytes".into(),
            ));
        }

        Ok(Self { raw, revm: Arc::new(revm) })
    }

    /// Returns true if the received BAL contains no accounts.
    pub fn is_empty(&self) -> bool {
        self.revm.accounts.is_empty()
    }

    /// Returns the number of accounts in the received BAL.
    pub fn len(&self) -> usize {
        self.revm.accounts.len()
    }

    /// Returns the revm BAL used by speculative workers.
    pub fn revm(&self) -> Arc<RevmBal> {
        Arc::clone(&self.revm)
    }

    /// Returns the revm BAL by reference.
    pub fn revm_ref(&self) -> &RevmBal {
        &self.revm
    }

    /// Returns the original raw BAL bytes.
    pub const fn as_raw_bal(&self) -> &RawBal {
        &self.raw
    }

    /// Validates the BAL item-cost bound against the block gas limit.
    pub fn validate_gas_limit(&self, gas_limit: u64) -> Result<(), ConsensusError> {
        let items = self.total_bal_items();
        if items > gas_limit / alloy_eip7928::constants::ITEM_COST as u64 {
            return Err(BlockAccessListGasError::new(items, gas_limit).into())
        }
        Ok(())
    }

    fn total_bal_items(&self) -> u64 {
        self.revm.accounts.values().map(|account| 1 + account.storage.storage.len() as u64).sum()
    }
}

fn decode_revm_bal(buf: &mut &[u8]) -> Result<RevmBal, RlpError> {
    let mut payload = Header::decode_bytes(buf, true)?;
    let mut accounts = Vec::new();
    while !payload.is_empty() {
        accounts.push(decode_account(&mut payload)?);
    }
    Ok(RevmBal { accounts: AddressIndexMap::from_iter(accounts) })
}

fn decode_account(buf: &mut &[u8]) -> Result<(Address, AccountBal), RlpError> {
    let mut payload = Header::decode_bytes(buf, true)?;
    let address = Address::decode(&mut payload)?;
    let storage_changes = Header::decode_bytes(&mut payload, true)?;
    let storage_reads = Header::decode_bytes(&mut payload, true)?;
    let balance_changes = Header::decode_bytes(&mut payload, true)?;
    let nonce_changes = Header::decode_bytes(&mut payload, true)?;
    let code_changes = Header::decode_bytes(&mut payload, true)?;
    require_empty(payload)?;

    Ok((
        address,
        AccountBal {
            account_info: AccountInfoBal {
                nonce: BalWrites { writes: decode_nonce_writes(nonce_changes)? },
                balance: BalWrites { writes: decode_u256_writes(balance_changes)? },
                code: BalWrites { writes: decode_code_writes(code_changes)? },
            },
            storage: decode_storage(storage_changes, storage_reads)?,
        },
    ))
}

fn decode_storage(
    mut storage_changes: &[u8],
    mut storage_reads: &[u8],
) -> Result<StorageBal, RlpError> {
    let mut storage = StorageBal::default();

    while !storage_changes.is_empty() {
        let mut payload = Header::decode_bytes(&mut storage_changes, true)?;
        let slot = U256::decode(&mut payload)?;
        let changes = Header::decode_bytes(&mut payload, true)?;
        require_empty(payload)?;
        storage.storage.insert(slot, BalWrites { writes: decode_u256_writes(changes)? });
    }

    while !storage_reads.is_empty() {
        let slot = U256::decode(&mut storage_reads)?;
        storage.storage.insert(slot, BalWrites::default());
    }

    Ok(storage)
}

fn decode_u256_writes(mut payload: &[u8]) -> Result<Vec<(BlockAccessIndex, U256)>, RlpError> {
    let mut writes = Vec::new();
    while !payload.is_empty() {
        let mut change = Header::decode_bytes(&mut payload, true)?;
        let index = BlockAccessIndex::decode(&mut change)?;
        let value = U256::decode(&mut change)?;
        require_empty(change)?;
        writes.push((index, value));
    }
    Ok(writes)
}

fn decode_nonce_writes(mut payload: &[u8]) -> Result<Vec<(BlockAccessIndex, u64)>, RlpError> {
    let mut writes = Vec::new();
    while !payload.is_empty() {
        let mut change = Header::decode_bytes(&mut payload, true)?;
        let index = BlockAccessIndex::decode(&mut change)?;
        let value = u64::decode(&mut change)?;
        require_empty(change)?;
        writes.push((index, value));
    }
    Ok(writes)
}

fn decode_code_writes(
    mut payload: &[u8],
) -> Result<Vec<(BlockAccessIndex, (B256, Bytecode))>, RlpError> {
    let mut writes = Vec::new();
    while !payload.is_empty() {
        let mut change = Header::decode_bytes(&mut payload, true)?;
        let index = BlockAccessIndex::decode(&mut change)?;
        let raw = Bytes::decode(&mut change)?;
        require_empty(change)?;

        let bytecode =
            Bytecode::new_raw_checked(raw).map_err(|_| RlpError::Custom("invalid bytecode"))?;
        writes.push((index, (bytecode.hash_slow(), bytecode)));
    }
    Ok(writes)
}

fn require_empty(payload: &[u8]) -> Result<(), RlpError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(RlpError::UnexpectedLength)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        AccountChanges, BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange,
    };

    #[test]
    fn direct_decode_matches_revm_clone_from_alloy() {
        let account = AccountChanges::new(Address::from([0x11; 20]))
            .with_storage_change(SlotChanges::new(
                U256::from(1),
                vec![
                    StorageChange::new(BlockAccessIndex::new(0), U256::from(0x10)),
                    StorageChange::new(BlockAccessIndex::new(2), U256::from(0x20)),
                ],
            ))
            .with_storage_read(U256::from(2))
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(1000)))
            .with_nonce_change(NonceChange::new(BlockAccessIndex::new(2), 7))
            .with_code_change(CodeChange::new(
                BlockAccessIndex::new(3),
                Bytes::from_static(&[0x60, 0x00, 0x60, 0x00, 0xf3]),
            ));
        let read_only_account =
            AccountChanges::new(Address::from([0x22; 20])).with_storage_read(U256::from(3));
        let block_access_list = vec![account, read_only_account];
        let alloy_bal = alloy_eip7928::bal::Bal::new(block_access_list.clone());
        let raw = alloy_rlp::encode(&alloy_bal).into();

        let expected = RevmBal::clone_from_alloy(&block_access_list).expect("valid alloy BAL");
        let received = ReceivedBal::from_rlp_bytes(raw).expect("valid raw BAL");

        assert_eq!(received.revm_ref(), &expected);
    }
}
