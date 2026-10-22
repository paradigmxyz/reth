//! Read-only parent post-state beneath `CachedReads` and the child executor's mutable `State`.

use alloy_eip7928::bal::DecodedBal;
use alloy_primitives::{Address, B256, U256};
use revm::{
    bytecode::Bytecode,
    state::{bal::Bal, AccountInfo},
    DatabaseRef,
};
use std::{cell::Cell, sync::Arc};

pub(super) type SharedBal = Arc<DecodedBal<Arc<Bal>>>;

/// The provider must expose the post-state of the same parent as `bal`.
pub(super) struct ParentBalDb<'a, DB> {
    db: DB,
    bal: Option<&'a Bal>,
    reads: ParentBalReads,
}

impl<'a, DB> ParentBalDb<'a, DB> {
    pub(super) fn new(db: DB, bal: Option<&'a Bal>) -> Self {
        Self { db, bal, reads: Default::default() }
    }
}

impl<DB> std::fmt::Debug for ParentBalDb<'_, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParentBalDb").field("has_bal", &self.bal.is_some()).finish_non_exhaustive()
    }
}

impl<DB: DatabaseRef> DatabaseRef for ParentBalDb<'_, DB> {
    type Error = DB::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let Some(account) = self.bal.and_then(|bal| bal.accounts.get(&address)) else {
            self.reads.provider_accounts.set(self.reads.provider_accounts.get() + 1);
            return self.db.basic_ref(address)
        };
        // Select the last write directly, including post-execution writes. Indexed BAL reads
        // are exclusive and positioning at the post-execution index would miss its writes.
        let balance = account.balance.writes.last().map(|(_, value)| *value);
        let nonce = account.nonce.writes.last().map(|(_, value)| *value);
        let code = account.code.writes.last().map(|(_, value)| value);

        // BALs do not encode account existence or storage-wide clears. Only a complete,
        // nonempty account can be reconstructed without consulting the parent state provider.
        // Empty/deleted accounts and partial fields must retain provider existence semantics.
        #[cfg(not(feature = "account-ext"))]
        if let (Some(balance), Some(nonce), Some((code_hash, code))) = (balance, nonce, code) {
            let info = AccountInfo {
                balance,
                nonce,
                code_hash: *code_hash,
                code: Some(code.clone()),
                ..Default::default()
            };
            if !info.is_empty() {
                self.reads.bal_accounts.set(self.reads.bal_accounts.get() + 1);
                return Ok(Some(info))
            }
        }
        self.reads.provider_accounts.set(self.reads.provider_accounts.get() + 1);
        let Some(mut info) = self.db.basic_ref(address)? else { return Ok(None) };
        if let Some(balance) = balance {
            info.balance = balance;
        }
        if let Some(nonce) = nonce {
            info.nonce = nonce;
        }
        if let Some((hash, code)) = code {
            info.code_hash = *hash;
            info.code = Some(code.clone());
        }
        Ok(Some(info))
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        if let Some(value) = self
            .bal
            .and_then(|bal| bal.accounts.get(&address))
            .and_then(|account| account.storage.storage.get(&index))
            .and_then(|writes| writes.writes.last())
        {
            self.reads.bal_slots.set(self.reads.bal_slots.get() + 1);
            return Ok(value.1)
        }
        // A read-only slot has no value, and an omitted slot may have been cleared by a
        // deletion. The underlying DB is already at the parent's post-state, not pre-state.
        self.reads.provider_slots.set(self.reads.provider_slots.get() + 1);
        self.db.storage_ref(address, index)
    }

    fn code_by_hash_ref(&self, hash: B256) -> Result<Bytecode, Self::Error> {
        // Changed code is supplied inline by basic_ref. Avoid an O(BAL size) hash scan here;
        // code referenced only by hash (including unchanged code) uses the parent's provider.
        self.reads.provider_code.set(self.reads.provider_code.get() + 1);
        self.db.code_by_hash_ref(hash)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        self.reads.provider_hashes.set(self.reads.provider_hashes.get() + 1);
        self.db.block_hash_ref(number)
    }
}

/// Aggregate on the execution worker; publish once instead of resolving metric handles per read.
#[derive(Debug, Default)]
struct ParentBalReads {
    bal_accounts: Cell<u64>,
    bal_slots: Cell<u64>,
    provider_accounts: Cell<u64>,
    provider_slots: Cell<u64>,
    provider_code: Cell<u64>,
    provider_hashes: Cell<u64>,
}

impl Drop for ParentBalReads {
    fn drop(&mut self) {
        for (kind, value) in [
            ("bal_accounts", self.bal_accounts.get()),
            ("bal_slots", self.bal_slots.get()),
            ("provider_accounts", self.provider_accounts.get()),
            ("provider_slots", self.provider_slots.get()),
            ("provider_code", self.provider_code.get()),
            ("provider_hashes", self.provider_hashes.get()),
        ] {
            if value != 0 {
                reth_metrics::metrics::counter!("builder.validation.parent_bal.reads", "kind" => kind).increment(value);
            }
        }
    }
}

/// A parent without a BAL commitment, or mismatched cached bytes, uses provider reads.
pub(super) fn committed_bal(bal: Option<SharedBal>, expected: Option<B256>) -> Option<SharedBal> {
    let expected = expected?;
    bal.filter(|bal| {
        let matches = bal.hash() == expected;
        if !matches {
            tracing::warn!(target: "rpc::validation", %expected, actual = %bal.hash(), "Parent BAL commitment mismatch; using state provider");
        }
        matches
    })
}
