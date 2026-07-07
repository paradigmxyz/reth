//! Sender-attached account proof for cold-EOA mempool admission (Phase 5).
//!
//! A partial-stateless validator only holds the network-level cache
//! ([`crate::NetworkStateCache`]). Mempool admission happens *before* a block (and
//! its sidecar) exists, so when a transaction's sender is **cold** (not in the
//! cache) the validator cannot read its nonce/balance/code to validate the tx.
//!
//! The design has the sender attach a
//! single-account Merkle proof — the exact shape of `eth_getProof` (EIP-1186) — to
//! the transaction. The validator verifies that proof against a recent canonical
//! state root, with **no state access of its own**, and admits the tx.
//!
//! This module implements the verification side (the generation side is vanilla
//! `eth_getProof`, reused via reth's [`AccountProof`]).

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use reth_trie_common::AccountProof;

/// EIP-7702 delegation designator prefix: `0xef01` (magic) ++ `0x00` (version),
/// followed by the 20-byte delegate address. Total length is 23 bytes.
const EIP7702_DELEGATION_PREFIX: [u8; 3] = [0xef, 0x01, 0x00];
/// `2 (magic) + 1 (version) + 20 (address)`.
const EIP7702_DELEGATION_LEN: usize = 23;

/// A sender-attached account proof: a single-account [`AccountProof`] anchored to a
/// recent canonical block, plus (only for a delegated sender) the EIP-7702
/// designator preimage.
///
/// The embedded [`AccountProof`] is reused from reth verbatim: it carries the
/// `storage_root` required to reconstruct the account leaf, already handles
/// non-existence proofs, and converts to/from EIP-1186 — so a vanilla node can be
/// the generation side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderAccountProof {
    /// Single-account proof (`storage_proofs` is empty — EOA admission needs no storage).
    /// `info == None` with an empty `storage_root` is a non-existence proof.
    pub proof: AccountProof,
    /// Canonical block this proof is anchored to. The `(number, hash, state_root)`
    /// triple makes anchoring reorg-safe (same principle as the Phase 1/2 cache anchor).
    pub anchor_block_number: u64,
    /// Hash of the anchor block header.
    pub anchor_block_hash: B256,
    /// State root the proof is verified against (must equal the anchor header's state root).
    pub anchor_state_root: B256,
    /// EIP-7702 delegation designator preimage (`0xef0100 ‖ addr`, 23 B). Required
    /// iff the proven `code_hash` is non-empty; `None` for a plain EOA.
    pub delegation_code: Option<Bytes>,
}

impl SenderAccountProof {
    /// Wrap an [`AccountProof`] (e.g. from `StateProofProvider::proof` or an
    /// `eth_getProof` response) with its anchor and optional delegation code.
    pub const fn new(
        proof: AccountProof,
        anchor_block_number: u64,
        anchor_block_hash: B256,
        anchor_state_root: B256,
        delegation_code: Option<Bytes>,
    ) -> Self {
        Self { proof, anchor_block_number, anchor_block_hash, anchor_state_root, delegation_code }
    }

    /// Verify the proof for admitting `input.tx_sender`'s transaction.
    ///
    /// **Precondition (caller's responsibility):** the validator must have already
    /// determined, against its *current head*, that the sender is **cold** (a cache
    /// miss). A warm sender must be validated from the cache, never from a
    /// possibly-stale attached proof — the "cold ⟹ unchanged within the window"
    /// invariant that makes the proof exact only holds for a confirmed-cold sender.
    ///
    /// `is_canonical(number, hash)` returns the canonical state root at that block,
    /// or `None` if `(number, hash)` is not on the canonical chain. It performs no
    /// state access — only a header lookup the validator already can do.
    ///
    /// On success returns the verified sender state (nonce/balance/delegation) that
    /// the caller then uses for the nonce/balance admission decision.
    pub fn verify(
        &self,
        input: &SenderAdmissionInput,
        is_canonical: impl Fn(u64, B256) -> Option<B256>,
    ) -> Result<VerifiedSender, SenderProofError> {
        // Rule 1 — sender binding (security-critical). `AccountProof::verify` only
        // proves the proof is valid *for its own address*; it says nothing about
        // whether that address is the tx sender. Without this bind, an attacker
        // attaches a well-funded third party's proof to a tx from their own
        // empty/cold account and passes the nonce/balance checks.
        if self.proof.address != input.tx_sender {
            return Err(SenderProofError::SenderMismatch {
                expected: input.tx_sender,
                proof: self.proof.address,
            });
        }

        // Rule 2 — anchor is canonical and its state root matches the claimed one.
        match is_canonical(self.anchor_block_number, self.anchor_block_hash) {
            None => {
                return Err(SenderProofError::AnchorNotCanonical {
                    number: self.anchor_block_number,
                    hash: self.anchor_block_hash,
                })
            }
            Some(canonical_root) if canonical_root != self.anchor_state_root => {
                return Err(SenderProofError::AnchorRootMismatch {
                    expected: self.anchor_state_root,
                    canonical: canonical_root,
                })
            }
            Some(_) => {}
        }

        // Rule 3 — freshness. The bound is the account eviction window and is
        // inclusive: for a confirmed-cold sender, "cold ⟹ unchanged within the
        // account window" makes an in-window anchor exact. A larger bound would accept
        // an anchor predating the account's last change → stale proof.
        if self.anchor_block_number > input.head_block_number {
            return Err(SenderProofError::AnchorInFuture {
                head: input.head_block_number,
                anchor: self.anchor_block_number,
            });
        }
        if input.head_block_number - self.anchor_block_number > input.account_window {
            return Err(SenderProofError::StaleAnchor {
                head: input.head_block_number,
                anchor: self.anchor_block_number,
                window: input.account_window,
            });
        }

        // Rule 4 — cryptographic MPT verification against the anchor state root.
        // A single-key proof, so none of the multiproof inline-node dedup concerns
        // apply. Non-existence (new/empty account) is handled inside `verify`.
        self.proof
            .verify(self.anchor_state_root)
            .map_err(|err| SenderProofError::ProofInvalid(err.to_string()))?;

        // From here `self.proof.info` is trusted (its leaf was just verified).
        let (state_nonce, state_balance, code_hash) = match &self.proof.info {
            Some(account) => (account.nonce, account.balance, account.get_bytecode_hash()),
            // Non-existence proof: the account is empty (nonce 0, balance 0, no code).
            None => (0, U256::ZERO, KECCAK_EMPTY),
        };

        // Rule 5 — EOA eligibility (EIP-3607 / EIP-7702), branched on code_hash.
        let is_delegated = if code_hash == KECCAK_EMPTY {
            // Plain EOA — proof alone is sufficient, nothing else required.
            false
        } else {
            // Delegated sender: the account leaf only commits to `code_hash`, so the
            // sender must reveal the designator preimage. We mirror reth's exact
            // `is_eip7702` check rather than assuming a non-empty code_hash implies a
            // delegation.
            let code = self
                .delegation_code
                .as_ref()
                .ok_or(SenderProofError::MissingDelegationCode { code_hash })?;
            // Format check (O(1)) before hashing: rejects oversized attacker-supplied
            // blobs without paying keccak over unbounded input.
            if !is_delegation_designator(code) {
                return Err(SenderProofError::NotDelegation);
            }
            let got = keccak256(code);
            if got != code_hash {
                return Err(SenderProofError::DelegationCodeHashMismatch {
                    expected: code_hash,
                    got,
                });
            }
            true
        };

        // Rule 6 — nonce staleness. The proven nonce is a lower bound (nonce only
        // increases), so a tx below it can be rejected outright. Balance is left to
        // the caller: for a genuinely cold account it is unchanged within the window
        // (Rule 3 invariant); the residual race is the standard mempool one, caught
        // by the builder and block validation.
        if input.tx_nonce < state_nonce {
            return Err(SenderProofError::NonceTooLow { tx: input.tx_nonce, state: state_nonce });
        }

        Ok(VerifiedSender {
            address: self.proof.address,
            nonce: state_nonce,
            balance: state_balance,
            is_delegated,
        })
    }
}

/// Inputs the validator supplies to [`SenderAccountProof::verify`], derived from the
/// transaction and the validator's own head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderAdmissionInput {
    /// Sender address recovered from the transaction signature (the caller does `ecrecover`).
    pub tx_sender: Address,
    /// Transaction nonce, for the lower-bound staleness check.
    pub tx_nonce: u64,
    /// The validator's current head block number.
    pub head_block_number: u64,
    /// The network's account eviction window (`LastNBlocksPolicy` account window).
    ///
    /// This is **not** a free knob: it is the freshness bound, and it must be exactly
    /// the network's account window. The bound cannot be larger (a looser bound accepts
    /// an anchor predating the account's last change → stale proof), and a
    /// smaller value would needlessly reject fresh proofs. It is passed in rather than a
    /// module constant only because the window lives in the cache config
    /// ([`crate::LastNBlocksPolicy`]), which this verification module does not depend on.
    pub account_window: u64,
}

/// The verified sender state returned on successful admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSender {
    /// Sender address (equals the tx sender, per Rule 1).
    pub address: Address,
    /// Account nonce proven at the anchor (a lower bound on the true nonce).
    pub nonce: u64,
    /// Account balance proven at the anchor.
    pub balance: U256,
    /// Whether the sender is an EIP-7702 delegated EOA.
    pub is_delegated: bool,
}

/// Reasons a [`SenderAccountProof`] can fail admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderProofError {
    /// The proof's address is not the transaction sender (Rule 1).
    SenderMismatch {
        /// The recovered tx sender.
        expected: Address,
        /// The address the proof is for.
        proof: Address,
    },
    /// The anchor `(number, hash)` is not on the canonical chain (Rule 2).
    AnchorNotCanonical {
        /// Anchor block number.
        number: u64,
        /// Anchor block hash.
        hash: B256,
    },
    /// The anchor state root does not match the canonical header's state root (Rule 2).
    AnchorRootMismatch {
        /// The root claimed by the proof.
        expected: B256,
        /// The canonical root at the anchor block.
        canonical: B256,
    },
    /// The anchor block is newer than the validator's head (Rule 3).
    AnchorInFuture {
        /// Validator head block number.
        head: u64,
        /// Anchor block number.
        anchor: u64,
    },
    /// The anchor is older than the freshness window (Rule 3).
    StaleAnchor {
        /// Validator head block number.
        head: u64,
        /// Anchor block number.
        anchor: u64,
        /// Freshness window in blocks.
        window: u64,
    },
    /// The Merkle proof failed to verify against the anchor state root (Rule 4).
    ProofInvalid(String),
    /// The sender has non-empty code but no delegation designator was attached (Rule 5).
    MissingDelegationCode {
        /// The proven (non-empty) code hash.
        code_hash: B256,
    },
    /// The attached designator does not hash to the proven code hash (Rule 5).
    DelegationCodeHashMismatch {
        /// The proven code hash.
        expected: B256,
        /// The hash of the attached code.
        got: B256,
    },
    /// The attached code hashes correctly but is not a valid EIP-7702 designator (Rule 5).
    /// A genuine contract can never be a valid sender under EIP-3607.
    NotDelegation,
    /// The transaction nonce is below the proven account nonce (Rule 6).
    NonceTooLow {
        /// The transaction nonce.
        tx: u64,
        /// The proven account nonce (lower bound).
        state: u64,
    },
}

impl core::fmt::Display for SenderProofError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl core::error::Error for SenderProofError {}

/// Whether `code` is a valid EIP-7702 delegation designator (`0xef0100 ‖ 20-byte addr`).
fn is_delegation_designator(code: &[u8]) -> bool {
    code.len() == EIP7702_DELEGATION_LEN && code[..3] == EIP7702_DELEGATION_PREFIX
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, U256};
    use reth_primitives_traits::Account;
    use reth_trie_common::{proof::ProofRetainer, HashBuilder, Nibbles, EMPTY_ROOT_HASH};

    const SENDER: Address = address!("0x1111111111111111111111111111111111111111");
    const ANCHOR_HASH: B256 = B256::repeat_byte(0xa1);
    const ACCOUNT_WINDOW: u64 = 60;

    /// Build a real single-account trie and return `(state_root, account_proof_nodes)`.
    fn build_account_proof(
        addr: Address,
        account: &Account,
        storage_root: B256,
    ) -> (B256, Vec<Bytes>) {
        let nibbles = Nibbles::unpack(keccak256(addr));
        let value = alloy_rlp::encode((*account).into_trie_account(storage_root));

        let mut hb = HashBuilder::default().with_proof_retainer(ProofRetainer::new(vec![nibbles]));
        hb.add_leaf(nibbles, &value);
        let root = hb.root();
        let proof = hb.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, b)| b).collect();
        (root, proof)
    }

    /// A funded plain EOA and its `SenderAccountProof` anchored at `anchor_number`.
    fn funded_eoa_proof(
        addr: Address,
        nonce: u64,
        anchor_number: u64,
    ) -> (SenderAccountProof, B256) {
        let account =
            Account { nonce, balance: U256::from(10u64).pow(U256::from(18)), bytecode_hash: None };
        let (root, proof_nodes) = build_account_proof(addr, &account, EMPTY_ROOT_HASH);
        let proof = AccountProof {
            address: addr,
            info: Some(account),
            proof: proof_nodes,
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        (SenderAccountProof::new(proof, anchor_number, ANCHOR_HASH, root, None), root)
    }

    fn input(sender: Address, tx_nonce: u64, head: u64) -> SenderAdmissionInput {
        SenderAdmissionInput {
            tx_sender: sender,
            tx_nonce,
            head_block_number: head,
            account_window: ACCOUNT_WINDOW,
        }
    }

    /// Canonical lookup that accepts the given anchor and root.
    fn canonical(anchor: u64, hash: B256, root: B256) -> impl Fn(u64, B256) -> Option<B256> {
        move |n, h| (n == anchor && h == hash).then_some(root)
    }

    #[test]
    fn accepts_valid_plain_eoa_proof() {
        let (sp, root) = funded_eoa_proof(SENDER, 5, 100);
        let verified = sp
            .verify(&input(SENDER, 7, 130), canonical(100, ANCHOR_HASH, root))
            .expect("valid proof should verify");
        assert_eq!(verified.address, SENDER);
        assert_eq!(verified.nonce, 5);
        assert!(!verified.is_delegated);
    }

    #[test]
    fn rejects_sender_mismatch() {
        // Attach a proof for SENDER to a tx from a different account.
        let (sp, root) = funded_eoa_proof(SENDER, 5, 100);
        let other = address!("0x2222222222222222222222222222222222222222");
        let err = sp.verify(&input(other, 7, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert_eq!(err, SenderProofError::SenderMismatch { expected: other, proof: SENDER });
    }

    #[test]
    fn rejects_non_canonical_anchor() {
        let (sp, root) = funded_eoa_proof(SENDER, 5, 100);
        // Lookup that never returns canonical.
        let err = sp.verify(&input(SENDER, 7, 130), |_, _| None).unwrap_err();
        assert!(matches!(err, SenderProofError::AnchorNotCanonical { .. }));
        // Wrong root at the anchor.
        let bad = B256::repeat_byte(0xbb);
        let err = sp.verify(&input(SENDER, 7, 130), canonical(100, ANCHOR_HASH, bad)).unwrap_err();
        assert!(matches!(err, SenderProofError::AnchorRootMismatch { .. }));
        let _ = root;
    }

    #[test]
    fn freshness_boundary_is_inclusive() {
        let (sp, root) = funded_eoa_proof(SENDER, 5, 100);
        let lookup = canonical(100, ANCHOR_HASH, root);
        // head - anchor == window  → accepted (inclusive).
        assert!(sp.verify(&input(SENDER, 7, 100 + ACCOUNT_WINDOW), &lookup).is_ok());
        // head - anchor == window + 1  → rejected.
        let err = sp.verify(&input(SENDER, 7, 100 + ACCOUNT_WINDOW + 1), &lookup).unwrap_err();
        assert!(matches!(err, SenderProofError::StaleAnchor { .. }));
    }

    #[test]
    fn rejects_tampered_proof_node() {
        let (mut sp, root) = funded_eoa_proof(SENDER, 5, 100);
        // Flip a byte in the (only) proof node so the root no longer matches.
        let mut bytes = sp.proof.proof[0].to_vec();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        sp.proof.proof[0] = bytes.into();
        let err = sp.verify(&input(SENDER, 7, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert!(matches!(err, SenderProofError::ProofInvalid(_)));
    }

    #[test]
    fn rejects_nonce_below_state() {
        let (sp, root) = funded_eoa_proof(SENDER, 5, 100);
        // tx nonce 4 < proven nonce 5.
        let err = sp.verify(&input(SENDER, 4, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert_eq!(err, SenderProofError::NonceTooLow { tx: 4, state: 5 });
    }

    #[test]
    fn accepts_non_existence_proof_for_empty_account() {
        // Empty trie: absence of the account is proven by an empty proof against the
        // empty root, with info = None.
        let proof = AccountProof {
            address: SENDER,
            info: None,
            proof: Vec::new(),
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        let sp = SenderAccountProof::new(proof, 100, ANCHOR_HASH, EMPTY_ROOT_HASH, None);
        let verified = sp
            .verify(&input(SENDER, 0, 130), canonical(100, ANCHOR_HASH, EMPTY_ROOT_HASH))
            .expect("non-existence proof should verify");
        assert_eq!(verified.nonce, 0);
        assert_eq!(verified.balance, U256::ZERO);
        assert!(!verified.is_delegated);
    }

    // --- EIP-7702 delegated sender branch ---

    /// A delegated EOA (`code_hash = keccak(designator)`) and its proof + designator.
    fn delegated_proof(
        delegate: Address,
        designator_override: Option<Bytes>,
    ) -> (SenderAccountProof, B256) {
        let mut designator = Vec::with_capacity(EIP7702_DELEGATION_LEN);
        designator.extend_from_slice(&EIP7702_DELEGATION_PREFIX);
        designator.extend_from_slice(delegate.as_slice());
        let designator: Bytes = designator.into();
        let code_hash = keccak256(&designator);

        let account =
            Account { nonce: 3, balance: U256::from(500u64), bytecode_hash: Some(code_hash) };
        let (root, proof_nodes) = build_account_proof(SENDER, &account, EMPTY_ROOT_HASH);
        let proof = AccountProof {
            address: SENDER,
            info: Some(account),
            proof: proof_nodes,
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        let delegation_code = Some(designator_override.unwrap_or(designator));
        (SenderAccountProof::new(proof, 100, ANCHOR_HASH, root, delegation_code), root)
    }

    #[test]
    fn accepts_delegated_sender_with_valid_designator() {
        let delegate = address!("0x3333333333333333333333333333333333333333");
        let (sp, root) = delegated_proof(delegate, None);
        let verified = sp
            .verify(&input(SENDER, 4, 130), canonical(100, ANCHOR_HASH, root))
            .expect("valid delegated sender should verify");
        assert!(verified.is_delegated);
    }

    #[test]
    fn rejects_delegated_sender_without_designator() {
        let delegate = address!("0x3333333333333333333333333333333333333333");
        let (mut sp, root) = delegated_proof(delegate, None);
        sp.delegation_code = None;
        let err = sp.verify(&input(SENDER, 4, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert!(matches!(err, SenderProofError::MissingDelegationCode { .. }));
    }

    #[test]
    fn rejects_designator_hash_mismatch() {
        let delegate = address!("0x3333333333333333333333333333333333333333");
        // Attach a *different* (but well-formed) designator that won't hash to code_hash.
        let mut wrong = Vec::new();
        wrong.extend_from_slice(&EIP7702_DELEGATION_PREFIX);
        wrong.extend_from_slice(address!("0x4444444444444444444444444444444444444444").as_slice());
        let (sp, root) = delegated_proof(delegate, Some(wrong.into()));
        let err = sp.verify(&input(SENDER, 4, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert!(matches!(err, SenderProofError::DelegationCodeHashMismatch { .. }));
    }

    #[test]
    fn rejects_non_delegation_code() {
        // A real contract: code_hash matches attached code, but it is not a 0xef0100
        // designator. Must be rejected (EIP-3607).
        let contract_code: Bytes = vec![0x60, 0x00, 0x60, 0x00].into(); // PUSH1 0 PUSH1 0
        let code_hash = keccak256(&contract_code);
        let account =
            Account { nonce: 1, balance: U256::from(500u64), bytecode_hash: Some(code_hash) };
        let (root, proof_nodes) = build_account_proof(SENDER, &account, EMPTY_ROOT_HASH);
        let proof = AccountProof {
            address: SENDER,
            info: Some(account),
            proof: proof_nodes,
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        let sp = SenderAccountProof::new(proof, 100, ANCHOR_HASH, root, Some(contract_code));
        let err = sp.verify(&input(SENDER, 2, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert_eq!(err, SenderProofError::NotDelegation);
    }

    #[test]
    fn is_delegation_designator_recognizes_format() {
        let mut d = Vec::new();
        d.extend_from_slice(&EIP7702_DELEGATION_PREFIX);
        d.extend_from_slice(&[0u8; 20]);
        assert!(is_delegation_designator(&d));
        assert!(!is_delegation_designator(&d[..22])); // too short
        assert!(!is_delegation_designator(&[0x60, 0x00])); // wrong prefix
    }

    // --- Multi-account trie: proofs with branch nodes (realistic shape) ---

    /// Build a trie holding `accounts` and retain the proof path for `target`
    /// (which may be absent → exclusion proof). Returns `(root, proof_nodes)`.
    fn build_multi_account_proof(
        accounts: &[(Address, Account)],
        target: Address,
    ) -> (B256, Vec<Bytes>) {
        let target_nibbles = Nibbles::unpack(keccak256(target));

        // HashBuilder requires leaves in ascending key order.
        let mut leaves: Vec<(Nibbles, Vec<u8>)> = accounts
            .iter()
            .map(|(addr, account)| {
                (
                    Nibbles::unpack(keccak256(addr)),
                    alloy_rlp::encode(account.into_trie_account(EMPTY_ROOT_HASH)),
                )
            })
            .collect();
        leaves.sort_by(|a, b| a.0.cmp(&b.0));

        let mut hb =
            HashBuilder::default().with_proof_retainer(ProofRetainer::new(vec![target_nibbles]));
        for (nibbles, value) in &leaves {
            hb.add_leaf(*nibbles, value);
        }
        let root = hb.root();
        let proof = hb.take_proof_nodes().into_nodes_sorted().into_iter().map(|(_, b)| b).collect();
        (root, proof)
    }

    /// A handful of accounts so the trie has branch nodes, not a single collapsed leaf.
    fn crowd() -> Vec<(Address, Account)> {
        (1u8..=8)
            .map(|i| {
                (
                    Address::repeat_byte(i),
                    Account {
                        nonce: i as u64,
                        balance: U256::from(1000u64 * i as u64),
                        bytecode_hash: None,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn accepts_proof_from_multi_account_trie() {
        let accounts = crowd();
        let (target, target_account) = accounts[3];
        let (root, proof_nodes) = build_multi_account_proof(&accounts, target);
        assert!(proof_nodes.len() > 1, "multi-account trie should yield a multi-node proof");

        let proof = AccountProof {
            address: target,
            info: Some(target_account),
            proof: proof_nodes,
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        let sp = SenderAccountProof::new(proof, 100, ANCHOR_HASH, root, None);
        let verified = sp
            .verify(&input(target, target_account.nonce, 130), canonical(100, ANCHOR_HASH, root))
            .expect("multi-node proof should verify");
        assert_eq!(verified.nonce, target_account.nonce);
        assert_eq!(verified.balance, target_account.balance);
    }

    #[test]
    fn accepts_exclusion_proof_in_non_empty_trie() {
        // The realistic empty-account case: the trie holds other accounts, and the
        // sender's absence is proven by the retained path (exclusion proof).
        let accounts = crowd();
        let absent = Address::repeat_byte(0xee);
        assert!(accounts.iter().all(|(a, _)| *a != absent));
        let (root, proof_nodes) = build_multi_account_proof(&accounts, absent);
        assert!(!proof_nodes.is_empty(), "exclusion proof must carry the path nodes");

        let proof = AccountProof {
            address: absent,
            info: None,
            proof: proof_nodes,
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        let sp = SenderAccountProof::new(proof, 100, ANCHOR_HASH, root, None);
        let verified = sp
            .verify(&input(absent, 0, 130), canonical(100, ANCHOR_HASH, root))
            .expect("exclusion proof in a non-empty trie should verify");
        assert_eq!(verified.nonce, 0);
        assert_eq!(verified.balance, U256::ZERO);
    }

    #[test]
    fn rejects_forged_exclusion_proof_for_existing_account() {
        // An account that EXISTS (with nonce 5) cannot be passed off as empty: claiming
        // info=None with the same path nodes must fail cryptographically.
        let accounts = crowd();
        let (target, _) = accounts[3];
        let (root, proof_nodes) = build_multi_account_proof(&accounts, target);

        let proof = AccountProof {
            address: target,
            info: None, // forged: claims non-existence
            proof: proof_nodes,
            storage_root: EMPTY_ROOT_HASH,
            storage_proofs: Vec::new(),
        };
        let sp = SenderAccountProof::new(proof, 100, ANCHOR_HASH, root, None);
        let err = sp.verify(&input(target, 0, 130), canonical(100, ANCHOR_HASH, root)).unwrap_err();
        assert!(matches!(err, SenderProofError::ProofInvalid(_)));
    }
}
