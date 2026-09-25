//! Structural public-mempool policy for EIP-8141 validation prefixes and EIP-8272 recent roots.

use alloy_consensus::TxEip8141;
use alloy_eips::{
    eip8141::{Frame, FrameMode, MAX_VERIFY_GAS, MAX_VERIFY_STATE_GAS},
    eip8272::{
        MAX_RECENT_ROOT_REFERENCES, RECENT_ROOT_ADDRESS, RECENT_ROOT_ENTRY_DOMAIN,
        RECENT_ROOT_LENGTH, RECENT_ROOT_STORAGE_DOMAIN, RECENT_ROOT_TUPLE_BYTES,
    },
};
use alloy_primitives::{keccak256, Address, B256, U256};

/// One root reference declared by an EIP-8272 verifier frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecentRootReference {
    /// Application-defined root source.
    pub source_id: B256,
    /// Consensus slot in which the root was written.
    pub slot: u64,
    /// Opaque root value committed by the source.
    pub root: B256,
}

impl RecentRootReference {
    /// The EIP-8272 storage dependency for this reference.
    pub fn dependency(self) -> RecentRootDependency {
        let index = self.slot % RECENT_ROOT_LENGTH;
        let mut storage_preimage = [0u8; 72];
        storage_preimage[..32].copy_from_slice(RECENT_ROOT_STORAGE_DOMAIN.as_slice());
        storage_preimage[32..64].copy_from_slice(self.source_id.as_slice());
        storage_preimage[64..].copy_from_slice(&index.to_be_bytes());

        let mut entry_preimage = [0u8; 104];
        entry_preimage[..32].copy_from_slice(RECENT_ROOT_ENTRY_DOMAIN.as_slice());
        entry_preimage[32..64].copy_from_slice(self.source_id.as_slice());
        entry_preimage[64..72].copy_from_slice(&self.slot.to_be_bytes());
        entry_preimage[72..].copy_from_slice(self.root.as_slice());

        RecentRootDependency {
            storage_key: U256::from_be_slice(keccak256(storage_preimage).as_slice()),
            entry_hash: keccak256(entry_preimage),
            expires_at_slot: self.slot.saturating_add(RECENT_ROOT_LENGTH),
        }
    }
}

/// A distinct recent-root storage predicate retained for public-pool revalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecentRootDependency {
    /// Storage slot in the recent-root contract.
    pub storage_key: U256,
    /// Expected committed `(source_id, slot, root)` entry.
    pub entry_hash: B256,
    /// First `current_slot` for which this dependency is expired.
    pub expires_at_slot: u64,
}

/// The optional canonical EIP-8272 verifier at the front of a validation prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentRootVerifier {
    /// Position in the complete frame list.
    pub index: usize,
    /// Tuple values committed by the transaction's signed frame data.
    pub references: Vec<RecentRootReference>,
}

impl RecentRootVerifier {
    /// Distinct state predicates that must remain valid while the transaction is in the pool.
    pub fn dependencies(&self) -> Vec<RecentRootDependency> {
        let mut dependencies = Vec::with_capacity(self.references.len());
        for reference in self.references.iter().copied() {
            let dependency = reference.dependency();
            if !dependencies.contains(&dependency) {
                dependencies.push(dependency);
            }
        }
        dependencies
    }
}

/// The structurally recognized public-mempool validation prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameValidationPolicy {
    /// Exclusive end of the validation prefix in `tx.frames`.
    pub prefix_end: usize,
    /// Index of the optional deploy frame.
    pub deploy_index: Option<usize>,
    /// Index of the optional canonical expiry frame.
    pub expiry_index: Option<usize>,
    /// Optional canonical recent-root verifier immediately following the expiry verifier.
    pub recent_root: Option<RecentRootVerifier>,
    /// Sum of execution gas limits in the prefix.
    pub declared_execution_gas: u64,
    /// Sum of state gas limits in the prefix.
    pub state_gas: u64,
}

impl FrameValidationPolicy {
    /// Recognizes one of the four public validation-prefix shapes.
    ///
    /// This is deliberately structural: approval success and receipt semantics are checked by
    /// execution later, and are not inferred from frame flags here.
    pub fn new(tx: &TxEip8141, signature_validation_gas: u64) -> Result<Self, &'static str> {
        let mut start = 0;
        let expiry_index = if tx.frames.first().is_some_and(Frame::is_expiry_verifier) {
            let frame = &tx.frames[0];
            if !frame.has_valid_expiry_verifier_fields() {
                return Err("invalid expiry verifier frame");
            }
            start = 1;
            Some(0)
        } else {
            None
        };

        let recent_root = match tx.frames.get(start) {
            Some(frame)
                if frame.mode == FrameMode::Verify &&
                    frame.target_address() == Some(RECENT_ROOT_ADDRESS) =>
            {
                let verifier = parse_recent_root_verifier(frame, start)?;
                start += 1;
                Some(verifier)
            }
            _ => None,
        };

        let rest = &tx.frames[start..];
        let (prefix_len, deploy_index) = match rest {
            [self_verify, ..] if is_sender_verify(self_verify, tx.sender, 3) => (1, None),
            [deploy, self_verify, ..]
                if is_deploy(deploy) && is_sender_verify(self_verify, tx.sender, 3) =>
            {
                (2, Some(start))
            }
            [only, pay, ..] if is_sender_verify(only, tx.sender, 2) && is_pay(pay) => (2, None),
            [deploy, only, pay, ..]
                if is_deploy(deploy) && is_sender_verify(only, tx.sender, 2) && is_pay(pay) =>
            {
                (3, Some(start))
            }
            _ => return Err("unrecognized validation prefix"),
        };

        let prefix_end = start + prefix_len;
        if tx.frames[prefix_end..].iter().any(|frame| frame.mode == FrameMode::Verify) {
            return Err("verify frame after validation prefix");
        }

        let mut declared_execution_gas: u64 = 0;
        let mut state_gas: u64 = 0;
        for frame in &tx.frames[..prefix_end] {
            declared_execution_gas = declared_execution_gas
                .checked_add(frame.limits.execution)
                .ok_or("execution gas overflows u64")?;
            state_gas =
                state_gas.checked_add(frame.limits.state).ok_or("state gas overflows u64")?;
            if frame.flags & 0x04 != 0 {
                return Err("atomic frame in validation prefix");
            }
        }
        if declared_execution_gas
            .checked_add(signature_validation_gas)
            .ok_or("verification gas overflows u64")? >
            MAX_VERIFY_GAS
        {
            return Err("verification gas budget exceeded");
        }
        if state_gas > MAX_VERIFY_STATE_GAS {
            return Err("state gas budget exceeded");
        }

        Ok(Self {
            prefix_end,
            deploy_index,
            expiry_index,
            recent_root,
            declared_execution_gas,
            state_gas,
        })
    }
}

fn parse_recent_root_verifier(
    frame: &Frame,
    index: usize,
) -> Result<RecentRootVerifier, &'static str> {
    if frame.flags != 0 || !frame.value.is_zero() || frame.limits.state != 0 {
        return Err("invalid recent root verifier frame")
    }
    let data = frame.data.as_ref();
    if !(RECENT_ROOT_TUPLE_BYTES..=MAX_RECENT_ROOT_REFERENCES * RECENT_ROOT_TUPLE_BYTES)
        .contains(&data.len()) ||
        data.len() % RECENT_ROOT_TUPLE_BYTES != 0
    {
        return Err("invalid recent root verifier data")
    }

    let mut references = Vec::with_capacity(data.len() / RECENT_ROOT_TUPLE_BYTES);
    for tuple in data.chunks_exact(RECENT_ROOT_TUPLE_BYTES) {
        let slot = u64::from_be_bytes(tuple[32..40].try_into().expect("fixed tuple width"));
        if slot.checked_add(RECENT_ROOT_LENGTH).is_none() {
            return Err("recent root slot overflows expiry")
        }
        references.push(RecentRootReference {
            source_id: B256::from_slice(&tuple[..32]),
            slot,
            root: B256::from_slice(&tuple[40..]),
        });
    }

    Ok(RecentRootVerifier { index, references })
}

fn is_deploy(frame: &Frame) -> bool {
    frame.mode == FrameMode::Default && frame.flags == 0
}

fn is_sender_verify(frame: &Frame, sender: Address, flags: u8) -> bool {
    frame.mode == FrameMode::Verify &&
        frame.flags == flags &&
        frame.target_address().is_none_or(|target| target == sender)
}

fn is_pay(frame: &Frame) -> bool {
    frame.mode == FrameMode::Verify && frame.flags == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip8141::{FrameAddress, FrameLimits, EXPIRY_DATA_LENGTH, EXPIRY_VERIFIER};
    use alloy_primitives::{b256, Bytes, U256};

    fn frame(mode: FrameMode, flags: u8, target: FrameAddress) -> Frame {
        Frame {
            mode,
            flags,
            target,
            limits: FrameLimits { execution: 10, state: 20 },
            value: U256::ZERO,
            data: Bytes::new(),
        }
    }

    fn tx(frames: Vec<Frame>) -> TxEip8141 {
        TxEip8141 { sender: Address::repeat_byte(0x11), frames, ..Default::default() }
    }

    fn sender() -> FrameAddress {
        Address::repeat_byte(0x11).into()
    }

    fn expiry_frame() -> Frame {
        let mut expiry = frame(FrameMode::Verify, 0, EXPIRY_VERIFIER.into());
        expiry.data = Bytes::from(vec![0; EXPIRY_DATA_LENGTH]);
        expiry.limits.state = 0;
        expiry
    }

    fn recent_root_frame(references: &[(B256, u64, B256)]) -> Frame {
        let mut data = Vec::with_capacity(references.len() * RECENT_ROOT_TUPLE_BYTES);
        for (source_id, slot, root) in references {
            data.extend_from_slice(source_id.as_slice());
            data.extend_from_slice(&slot.to_be_bytes());
            data.extend_from_slice(root.as_slice());
        }
        let mut frame = frame(FrameMode::Verify, 0, RECENT_ROOT_ADDRESS.into());
        frame.limits.state = 0;
        frame.data = Bytes::from(data);
        frame
    }

    #[test]
    fn recognizes_all_four_shapes() {
        let s = sender();
        let cases = [
            (vec![frame(FrameMode::Verify, 3, s.clone())], 1),
            (
                vec![
                    frame(FrameMode::Default, 0, FrameAddress::default()),
                    frame(FrameMode::Verify, 3, s.clone()),
                ],
                2,
            ),
            (
                vec![
                    frame(FrameMode::Verify, 2, s.clone()),
                    frame(FrameMode::Verify, 1, FrameAddress::default()),
                ],
                2,
            ),
            (
                vec![
                    frame(FrameMode::Default, 0, FrameAddress::default()),
                    frame(FrameMode::Verify, 2, s),
                    frame(FrameMode::Verify, 1, FrameAddress::default()),
                ],
                3,
            ),
        ];
        for (frames, expected) in cases {
            assert_eq!(FrameValidationPolicy::new(&tx(frames), 1).unwrap().prefix_end, expected);
        }
    }

    #[test]
    fn expiry_and_suffix_rules() {
        let expiry = expiry_frame();
        let s = sender();
        let mut t = tx(vec![expiry, frame(FrameMode::Verify, 3, s)]);
        let p = FrameValidationPolicy::new(&t, 1).unwrap();
        assert_eq!(p.expiry_index, Some(0));
        t.frames[0].limits.execution = MAX_VERIFY_GAS;
        assert_eq!(FrameValidationPolicy::new(&t, 0), Err("verification gas budget exceeded"));
        t.frames.push(frame(FrameMode::Verify, 0, FrameAddress::default()));
        assert_eq!(FrameValidationPolicy::new(&t, 1), Err("verify frame after validation prefix"));
    }

    #[test]
    fn rejects_invalid_shape_and_budgets_without_overflow() {
        let s = sender();
        for bad in [
            vec![frame(FrameMode::Verify, 7, s.clone())],
            vec![frame(FrameMode::Verify, 3, Address::repeat_byte(0x22).into())],
            vec![frame(FrameMode::Verify, 2, s.clone())],
        ] {
            assert!(FrameValidationPolicy::new(&tx(bad), 1).is_err());
        }
        assert!(FrameValidationPolicy::new(
            &tx(vec![
                frame(FrameMode::Verify, 3, s.clone()),
                frame(FrameMode::Default, 0, FrameAddress::default()),
            ]),
            1
        )
        .is_ok());
        let mut t = tx(vec![frame(FrameMode::Verify, 3, s)]);
        t.frames[0].limits.execution = 99_999;
        assert!(FrameValidationPolicy::new(&t, 2).is_err());
        t.frames[0].limits.execution = u64::MAX;
        assert!(FrameValidationPolicy::new(&t, 0).is_err());
        t.frames[0].limits.execution = 10;
        t.frames[0].limits.state = MAX_VERIFY_STATE_GAS + 1;
        assert!(FrameValidationPolicy::new(&t, 0).is_err());
    }

    #[test]
    fn recognizes_recent_root_after_optional_expiry_for_every_account_shape() {
        let source_id = B256::repeat_byte(1);
        let root = B256::repeat_byte(2);
        let shapes = [
            vec![frame(FrameMode::Verify, 3, sender())],
            vec![
                frame(FrameMode::Default, 0, FrameAddress::default()),
                frame(FrameMode::Verify, 3, sender()),
            ],
            vec![
                frame(FrameMode::Verify, 2, sender()),
                frame(FrameMode::Verify, 1, FrameAddress::default()),
            ],
            vec![
                frame(FrameMode::Default, 0, FrameAddress::default()),
                frame(FrameMode::Verify, 2, sender()),
                frame(FrameMode::Verify, 1, FrameAddress::default()),
            ],
        ];

        for shape in shapes {
            for with_expiry in [false, true] {
                for with_recent_root in [false, true] {
                    let mut frames = Vec::new();
                    if with_expiry {
                        frames.push(expiry_frame());
                    }
                    if with_recent_root {
                        frames.push(recent_root_frame(&[(source_id, 7, root)]));
                    }
                    let expected_prefix_end = frames.len() + shape.len();
                    frames.extend(shape.clone());

                    let policy = FrameValidationPolicy::new(&tx(frames), 0).unwrap();
                    assert_eq!(policy.prefix_end, expected_prefix_end);
                    assert_eq!(policy.expiry_index, with_expiry.then_some(0));
                    assert_eq!(
                        policy.recent_root.as_ref().map(|verifier| verifier.index),
                        with_recent_root.then_some(if with_expiry { 1 } else { 0 })
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_malformed_and_misplaced_recent_root_verifiers() {
        let source_id = B256::repeat_byte(1);
        let root = B256::repeat_byte(2);
        let mut malformed = recent_root_frame(&[(source_id, 7, root)]);
        malformed.data = Bytes::new();
        assert_eq!(
            FrameValidationPolicy::new(
                &tx(vec![malformed, frame(FrameMode::Verify, 3, sender())]),
                0
            ),
            Err("invalid recent root verifier data")
        );

        let mut malformed = recent_root_frame(&[(source_id, 7, root)]);
        malformed.flags = 1;
        assert_eq!(
            FrameValidationPolicy::new(
                &tx(vec![malformed, frame(FrameMode::Verify, 3, sender())]),
                0
            ),
            Err("invalid recent root verifier frame")
        );

        let root_frame = recent_root_frame(&[(source_id, 7, root)]);
        assert!(FrameValidationPolicy::new(
            &tx(vec![frame(FrameMode::Verify, 3, sender()), root_frame.clone()]),
            0
        )
        .is_err());
        assert!(FrameValidationPolicy::new(
            &tx(vec![root_frame.clone(), expiry_frame(), frame(FrameMode::Verify, 3, sender())]),
            0
        )
        .is_err());
        assert!(FrameValidationPolicy::new(
            &tx(vec![
                root_frame.clone(),
                recent_root_frame(&[(source_id, 8, root)]),
                frame(FrameMode::Verify, 3, sender()),
            ]),
            0
        )
        .is_err());

        let mut too_many = Vec::new();
        for slot in 0..=MAX_RECENT_ROOT_REFERENCES {
            too_many.push((source_id, slot as u64, root));
        }
        assert_eq!(
            FrameValidationPolicy::new(
                &tx(vec![recent_root_frame(&too_many), frame(FrameMode::Verify, 3, sender())]),
                0
            ),
            Err("invalid recent root verifier data")
        );
    }

    #[test]
    fn derives_the_reference_vector_and_deduplicates_dependencies() {
        let reference = RecentRootReference {
            source_id: b256!("b9382d35273c75a50631a3e84d3c75ec9266e2b18c35a627e16cdbf26a18ca85"),
            slot: 1,
            root: b256!("0000000000000000000000000000000000000000000000000000000000000002"),
        };
        let dependency = reference.dependency();
        assert_eq!(
            dependency.entry_hash,
            b256!("0a0d1254c851be5a133b4c9a9e300f5602fc0f43dbe65aa6a66930d4ca0a51b8")
        );
        assert_eq!(
            dependency.storage_key,
            U256::from_be_bytes(
                b256!("5f027aa1cbe2df279bf6518edd4b44ea5409fd800189ec35224e10ab05e574c3").0
            )
        );
        assert_eq!(dependency.expires_at_slot, RECENT_ROOT_LENGTH + 1);

        let policy = FrameValidationPolicy::new(
            &tx(vec![
                recent_root_frame(&[(reference.source_id, reference.slot, reference.root); 2]),
                frame(FrameMode::Verify, 3, sender()),
            ]),
            0,
        )
        .unwrap();
        assert_eq!(policy.recent_root.unwrap().dependencies(), vec![dependency]);
    }

    #[test]
    fn recent_root_execution_limit_counts_toward_verify_budget() {
        let mut root = recent_root_frame(&[(B256::repeat_byte(1), 7, B256::repeat_byte(2))]);
        root.limits.execution = MAX_VERIFY_GAS;
        assert_eq!(
            FrameValidationPolicy::new(&tx(vec![root, frame(FrameMode::Verify, 3, sender())]), 0),
            Err("verification gas budget exceeded")
        );
    }
}
