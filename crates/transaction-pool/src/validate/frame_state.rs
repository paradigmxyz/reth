use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use alloy_primitives::{Address, TxHash, B256, U256};

/// State metadata captured while validating a frame transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameValidation {
    /// Frame sender.
    pub sender: Address,
    /// Transaction nonce.
    pub sender_nonce: u64,
    /// Sender nonce in the canonical state used for validation.
    pub state_nonce: u64,
    /// Sender balance at validation time.
    pub sender_balance: U256,
    /// Sender bytecode hash, if present.
    pub sender_code_hash: Option<B256>,
    /// Account paying for the frame.
    pub payer: Address,
    /// Maximum balance reservation for the frame.
    pub max_cost: U256,
    /// Payer balance at validation time.
    pub payer_balance: U256,
    /// Head against which this metadata was validated.
    pub head_hash: B256,
    /// State dependencies read by the frame.
    pub dependencies: FrameDependencies,
    /// Optional expiration timestamp.
    pub expires_at: Option<u64>,
    /// Whether this frame consumes the payer's exclusive pending slot.
    pub exclusive_payer: bool,
}

/// State addresses and slots observed by the validation prefix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameDependencies {
    /// Accounts whose state can affect validation.
    pub accounts: Vec<Address>,
    /// Accounts whose code can affect validation.
    pub code: Vec<Address>,
    /// Storage slots whose values can affect validation.
    pub storage: Vec<(Address, U256)>,
}

/// Indexed reservations for frame transactions.
#[derive(Debug, Default)]
pub struct FrameReservations {
    frames: HashMap<TxHash, Arc<FrameValidation>>,
    sender_nonce: HashMap<(Address, u64), TxHash>,
    payer: HashMap<Address, PayerUsage>,
    accounts: HashMap<Address, HashSet<TxHash>>,
    code: HashMap<Address, HashSet<TxHash>>,
    storage: HashMap<(Address, U256), HashSet<TxHash>>,
    storage_address: HashMap<Address, HashSet<TxHash>>,
    expiry: BTreeMap<u64, HashSet<TxHash>>,
}

/// Aggregate exposure held against one payer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PayerUsage {
    /// Canonical balance used by the currently admitted reservations.
    pub balance: U256,
    /// Sum of reserved frame costs.
    pub frame_cost: U256,
    /// Number of frames using the payer.
    pub frame_count: usize,
    /// Number of exclusive payer frames using this payer.
    pub exclusive_count: usize,
}

impl FrameReservations {
    /// Iterates validated transaction hashes without scanning ordinary transactions.
    pub fn hashes(&self) -> impl Iterator<Item = B256> + '_ {
        self.frames.keys().copied()
    }

    /// Iterates reservations in increasing expiry order for resource-pressure eviction.
    pub fn expiring_hashes(&self) -> impl Iterator<Item = B256> + '_ {
        self.expiry.values().flat_map(|hashes| hashes.iter().copied())
    }

    /// Atomically inserts a frame, optionally replacing its same-sender nonce.
    pub fn replace(
        &mut self,
        hash: TxHash,
        metadata: Arc<FrameValidation>,
        replaces: Option<B256>,
        ordinary_payer_cost: U256,
        current_head: B256,
    ) -> Result<(), &'static str> {
        if metadata.head_hash != current_head {
            return Err("stale head");
        }
        if self.frames.contains_key(&hash) {
            return Err("duplicate hash");
        }
        let old = replaces.and_then(|h| self.frames.get(&h).cloned());
        if replaces.is_some() && old.is_none() {
            return Err("replacement not found");
        }
        if let Some(old) = &old {
            if old.sender != metadata.sender || old.sender_nonce != metadata.sender_nonce {
                return Err("replacement sender nonce mismatch");
            }
        } else if self.sender_nonce.contains_key(&(metadata.sender, metadata.sender_nonce)) {
            return Err("sender nonce already reserved");
        }
        let usage = self.payer.get(&metadata.payer).copied().unwrap_or_default();
        let old_cost =
            old.as_ref().filter(|f| f.payer == metadata.payer).map_or(U256::ZERO, |f| f.max_cost);
        let frame_cost = usage
            .frame_cost
            .checked_sub(old_cost)
            .ok_or("reservation accounting error")?
            .checked_add(metadata.max_cost)
            .ok_or("payer cost overflow")?;
        let balance = metadata
            .payer_balance
            .checked_sub(ordinary_payer_cost)
            .ok_or("ordinary payer exposure exceeds balance")?;
        if frame_cost > balance {
            return Err("payer balance exceeded");
        }
        let other_count = usage
            .frame_count
            .checked_sub(usize::from(old.as_ref().is_some_and(|f| f.payer == metadata.payer)))
            .ok_or("reservation accounting error")?;
        let old_exclusive_count = usize::from(
            old.as_ref().is_some_and(|f| f.payer == metadata.payer && f.exclusive_payer),
        );
        let other_exclusive_count = usage
            .exclusive_count
            .checked_sub(old_exclusive_count)
            .ok_or("reservation accounting error")?;
        // Non-exclusive self-paid reservations may coexist; only exclusive reservations consume
        // the payer's one-pending slot.
        if metadata.exclusive_payer && other_exclusive_count != 0 {
            return Err("exclusive payer capacity exceeded");
        }
        self.remove_inner(replaces);
        self.frames.insert(hash, metadata.clone());
        self.sender_nonce.insert((metadata.sender, metadata.sender_nonce), hash);
        let entry = self.payer.entry(metadata.payer).or_default();
        entry.balance = metadata.payer_balance;
        entry.frame_cost = frame_cost;
        entry.frame_count = other_count + 1;
        entry.exclusive_count = other_exclusive_count + usize::from(metadata.exclusive_payer);
        for a in
            metadata.dependencies.accounts.iter().copied().chain([metadata.sender, metadata.payer])
        {
            self.accounts.entry(a).or_default().insert(hash);
        }
        for a in metadata.dependencies.code.iter().copied() {
            self.code.entry(a).or_default().insert(hash);
        }
        for s in metadata.dependencies.storage.iter().copied() {
            self.storage.entry(s).or_default().insert(hash);
        }
        for a in metadata.dependencies.storage.iter().map(|(address, _)| *address) {
            self.storage_address.entry(a).or_default().insert(hash);
        }
        if let Some(t) = metadata.expires_at {
            self.expiry.entry(t).or_default().insert(hash);
        }
        Ok(())
    }

    /// Removes a reservation if present.
    pub fn remove(&mut self, hash: &TxHash) {
        self.remove_inner(Some(*hash));
    }

    fn remove_inner(&mut self, hash: Option<B256>) {
        let Some(hash) = hash else { return };
        let Some(m) = self.frames.remove(&hash) else { return };
        self.sender_nonce.remove(&(m.sender, m.sender_nonce));
        if let Some(p) = self.payer.get_mut(&m.payer) {
            p.frame_cost = p.frame_cost.checked_sub(m.max_cost).unwrap_or(U256::ZERO);
            p.frame_count = p.frame_count.saturating_sub(1);
            if m.exclusive_payer {
                p.exclusive_count = p.exclusive_count.saturating_sub(1);
            }
            if p.frame_count == 0 {
                self.payer.remove(&m.payer);
            }
        }
        for a in m.dependencies.accounts.iter().copied().chain([m.sender, m.payer]) {
            if let Some(s) = self.accounts.get_mut(&a) {
                s.remove(&hash);
                if s.is_empty() {
                    self.accounts.remove(&a);
                }
            }
        }
        for a in &m.dependencies.code {
            if let Some(s) = self.code.get_mut(a) {
                s.remove(&hash);
                if s.is_empty() {
                    self.code.remove(a);
                }
            }
        }
        for s in &m.dependencies.storage {
            if let Some(v) = self.storage.get_mut(s) {
                v.remove(&hash);
                if v.is_empty() {
                    self.storage.remove(s);
                }
            }
        }
        for a in m.dependencies.storage.iter().map(|(address, _)| *address) {
            if let Some(v) = self.storage_address.get_mut(&a) {
                v.remove(&hash);
                if v.is_empty() {
                    self.storage_address.remove(&a);
                }
            }
        }
        if let Some(t) = m.expires_at &&
            let Some(v) = self.expiry.get_mut(&t)
        {
            v.remove(&hash);
            if v.is_empty() {
                self.expiry.remove(&t);
            }
        }
    }

    /// Returns indexed frame usage for a payer.
    pub fn payer_usage(&self, payer: &Address) -> PayerUsage {
        self.payer.get(payer).copied().unwrap_or_default()
    }
    /// Returns the total indexed frame-cost exposure for a payer.
    pub fn payer_exposure(&self, payer: &Address) -> U256 {
        self.payer_usage(payer).frame_cost
    }
    /// Returns reservations affected by changed state or expiration.
    pub fn affected(&self, changed: &FrameDependencies, timestamp: u64) -> HashSet<TxHash> {
        let mut out = HashSet::new();
        for a in &changed.accounts {
            if let Some(v) = self.accounts.get(a) {
                out.extend(v.iter().copied())
            }
            if let Some(v) = self.code.get(a) {
                out.extend(v.iter().copied())
            }
            if let Some(v) = self.storage_address.get(a) {
                out.extend(v.iter().copied())
            }
        }
        for a in &changed.code {
            if let Some(v) = self.code.get(a) {
                out.extend(v.iter().copied())
            }
        }
        for slot in &changed.storage {
            if let Some(v) = self.storage.get(slot) {
                out.extend(v.iter().copied())
            }
        }
        for (_, v) in self.expiry.range(..timestamp) {
            out.extend(v.iter().copied())
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(sender: u8, nonce: u64, payer: u8, cost: u64) -> FrameValidation {
        FrameValidation {
            sender: Address::repeat_byte(sender),
            sender_nonce: nonce,
            state_nonce: nonce,
            sender_balance: U256::MAX,
            sender_code_hash: None,
            payer: Address::repeat_byte(payer),
            max_cost: U256::from(cost),
            payer_balance: U256::from(10),
            head_hash: B256::ZERO,
            dependencies: FrameDependencies {
                accounts: vec![Address::repeat_byte(9), Address::repeat_byte(9)],
                code: vec![],
                storage: vec![],
            },
            expires_at: None,
            exclusive_payer: false,
        }
    }
    fn put(r: &mut FrameReservations, n: u8, m: FrameValidation) -> Result<(), &'static str> {
        r.replace(B256::repeat_byte(n), Arc::new(m), None, U256::ZERO, B256::ZERO)
    }

    #[test]
    fn reserve_overflow_and_balance() {
        let mut r = FrameReservations::default();
        let mut x = m(1, 0, 1, 10);
        assert!(put(&mut r, 1, x.clone()).is_ok());
        x.sender = Address::repeat_byte(2);
        assert_eq!(put(&mut r, 2, x), Err("payer balance exceeded"));

        let mut r = FrameReservations::default();
        let mut x = m(1, 0, 1, 0);
        x.max_cost = U256::MAX;
        x.payer_balance = U256::MAX;
        put(&mut r, 1, x).unwrap();
        let mut y = m(2, 0, 1, 1);
        y.payer_balance = U256::MAX;
        assert_eq!(put(&mut r, 2, y), Err("payer cost overflow"));
        assert_eq!(r.payer_exposure(&Address::repeat_byte(1)), U256::MAX);
    }

    #[test]
    fn stale_head_and_different_nonce_do_not_mutate_reservations() {
        let mut r = FrameReservations::default();
        put(&mut r, 1, m(1, 0, 1, 3)).unwrap();
        let mut stale = m(2, 0, 1, 2);
        stale.head_hash = B256::repeat_byte(7);
        assert_eq!(put(&mut r, 2, stale), Err("stale head"));
        assert!(put(&mut r, 3, m(1, 1, 2, 1)).is_ok());
        assert_eq!(r.payer_exposure(&Address::repeat_byte(1)), U256::from(3));
        assert_eq!(r.hashes().count(), 2);
    }

    #[test]
    fn account_deletion_invalidates_storage_dependencies() {
        let mut r = FrameReservations::default();
        let address = Address::repeat_byte(8);
        let slot = U256::from(5);
        let mut x = m(1, 0, 1, 1);
        x.dependencies.storage.push((address, slot));
        put(&mut r, 1, x).unwrap();
        let other_slot = FrameDependencies {
            storage: vec![(address, slot + U256::from(1))],
            ..Default::default()
        };
        assert!(r.affected(&other_slot, 0).is_empty());
        for changed in [
            FrameDependencies { accounts: vec![address], ..Default::default() },
            FrameDependencies { storage: vec![(address, slot)], ..Default::default() },
            FrameDependencies { accounts: vec![Address::repeat_byte(1)], ..Default::default() },
        ] {
            assert_eq!(r.affected(&changed, 0), HashSet::from([B256::repeat_byte(1)]));
            r.remove(&B256::repeat_byte(99));
        }
        r.remove(&B256::repeat_byte(1));
        let changed = FrameDependencies { accounts: vec![address], ..Default::default() };
        assert!(r.affected(&changed, u64::MAX).is_empty());
    }
    #[test]
    fn removal_preserves_exclusive_admission_invariants() {
        let mut r = FrameReservations::default();
        put(&mut r, 1, m(1, 0, 1, 1)).unwrap();
        let mut exclusive = m(3, 0, 1, 1);
        exclusive.exclusive_payer = true;
        put(&mut r, 2, exclusive.clone()).unwrap();
        assert_eq!(r.payer_usage(&exclusive.payer).exclusive_count, 1);

        r.remove(&B256::repeat_byte(1));
        assert_eq!(r.payer_usage(&exclusive.payer).frame_count, 1);
        assert_eq!(r.payer_usage(&exclusive.payer).exclusive_count, 1);
        let mut another_exclusive = m(4, 0, 1, 1);
        another_exclusive.exclusive_payer = true;
        assert_eq!(put(&mut r, 3, another_exclusive), Err("exclusive payer capacity exceeded"));

        r.remove(&B256::repeat_byte(2));
        assert_eq!(r.payer_usage(&exclusive.payer), PayerUsage::default());

        put(&mut r, 4, m(4, 0, 1, 1)).unwrap();
        let mut second_exclusive = m(5, 0, 1, 1);
        second_exclusive.exclusive_payer = true;
        put(&mut r, 5, second_exclusive).unwrap();
        r.remove(&B256::repeat_byte(5));
        assert_eq!(r.payer_usage(&exclusive.payer).exclusive_count, 0);
        assert_eq!(r.payer_usage(&exclusive.payer).frame_count, 1);
    }

    #[test]
    fn exclusive_cap_and_nonce_rules() {
        let mut r = FrameReservations::default();
        let mut x = m(1, 0, 1, 1);
        x.exclusive_payer = true;
        put(&mut r, 1, x).unwrap();
        assert!(put(&mut r, 2, m(2, 0, 1, 1)).is_ok());
        let mut second_exclusive = m(3, 0, 1, 1);
        second_exclusive.exclusive_payer = true;
        assert_eq!(put(&mut r, 3, second_exclusive), Err("exclusive payer capacity exceeded"));
        assert_eq!(put(&mut r, 3, m(1, 0, 2, 1)), Err("sender nonce already reserved"));
    }
    #[test]
    fn replacement_rollback_and_payer_shift() {
        let mut r = FrameReservations::default();
        put(&mut r, 1, m(1, 0, 1, 3)).unwrap();
        let mut x = m(1, 0, 2, 4);
        assert_eq!(
            r.replace(
                B256::repeat_byte(2),
                Arc::new(x.clone()),
                Some(B256::repeat_byte(1)),
                U256::from(7),
                B256::ZERO
            ),
            Err("payer balance exceeded")
        );
        assert_eq!(r.payer_exposure(&Address::repeat_byte(1)), U256::from(3));
        x.payer = Address::repeat_byte(2);
        r.replace(
            B256::repeat_byte(2),
            Arc::new(x),
            Some(B256::repeat_byte(1)),
            U256::ZERO,
            B256::ZERO,
        )
        .unwrap();
        assert_eq!(r.payer_exposure(&Address::repeat_byte(1)), U256::ZERO);
    }
    #[test]
    fn removal_and_affected_expiry() {
        let mut r = FrameReservations::default();
        let mut x = m(1, 0, 1, 1);
        x.expires_at = Some(5);
        put(&mut r, 1, x).unwrap();
        let d = FrameDependencies { accounts: vec![Address::repeat_byte(9)], ..Default::default() };
        assert!(r.affected(&d, 4).contains(&B256::repeat_byte(1)));
        assert!(r.affected(&FrameDependencies::default(), 6).contains(&B256::repeat_byte(1)));
        r.remove(&B256::repeat_byte(1));
        r.remove(&B256::repeat_byte(1));
        assert_eq!(r.payer_exposure(&Address::repeat_byte(1)), U256::ZERO);
    }
}
