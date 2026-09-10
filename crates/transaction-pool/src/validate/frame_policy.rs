//! Structural public-mempool policy for EIP-8141 validation prefixes.

use alloy_consensus::TxEip8141;
use alloy_eips::eip8141::{Frame, FrameMode, MAX_VERIFY_GAS, MAX_VERIFY_STATE_GAS};
use alloy_primitives::Address;

/// The structurally recognized public-mempool validation prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameValidationPolicy {
    /// Exclusive end of the validation prefix in `tx.frames`.
    pub prefix_end: usize,
    /// Index of the optional deploy frame.
    pub deploy_index: Option<usize>,
    /// Index of the optional canonical expiry frame.
    pub expiry_index: Option<usize>,
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

        Ok(Self { prefix_end, deploy_index, expiry_index, declared_execution_gas, state_gas })
    }
}

fn is_deploy(frame: &Frame) -> bool {
    frame.mode == FrameMode::Default && frame.flags == 0 && frame.has_valid_target_encoding()
}

fn is_sender_verify(frame: &Frame, sender: Address, flags: u8) -> bool {
    frame.mode == FrameMode::Verify &&
        frame.flags == flags &&
        frame.has_valid_target_encoding() &&
        frame.target_address().is_none_or(|target| target == sender)
}

fn is_pay(frame: &Frame) -> bool {
    frame.mode == FrameMode::Verify && frame.flags == 1 && frame.has_valid_target_encoding()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip8141::{FrameLimits, EXPIRY_DATA_LENGTH, EXPIRY_VERIFIER};
    use alloy_primitives::{Bytes, U256};

    fn frame(mode: FrameMode, flags: u8, target: Bytes) -> Frame {
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

    fn sender() -> Bytes {
        Bytes::copy_from_slice(Address::repeat_byte(0x11).as_slice())
    }

    #[test]
    fn recognizes_all_four_shapes() {
        let s = sender();
        let cases = [
            (vec![frame(FrameMode::Verify, 3, s.clone())], 1),
            (
                vec![
                    frame(FrameMode::Default, 0, Bytes::new()),
                    frame(FrameMode::Verify, 3, s.clone()),
                ],
                2,
            ),
            (
                vec![
                    frame(FrameMode::Verify, 2, s.clone()),
                    frame(FrameMode::Verify, 1, Bytes::new()),
                ],
                2,
            ),
            (
                vec![
                    frame(FrameMode::Default, 0, Bytes::new()),
                    frame(FrameMode::Verify, 2, s),
                    frame(FrameMode::Verify, 1, Bytes::new()),
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
        let mut expiry =
            frame(FrameMode::Verify, 0, Bytes::copy_from_slice(EXPIRY_VERIFIER.as_slice()));
        expiry.data = Bytes::from(vec![0; EXPIRY_DATA_LENGTH]);
        expiry.limits.state = 0;
        let s = sender();
        let mut t = tx(vec![expiry, frame(FrameMode::Verify, 3, s)]);
        let p = FrameValidationPolicy::new(&t, 1).unwrap();
        assert_eq!(p.expiry_index, Some(0));
        t.frames[0].limits.execution = MAX_VERIFY_GAS;
        assert_eq!(FrameValidationPolicy::new(&t, 0), Err("verification gas budget exceeded"));
        t.frames.push(frame(FrameMode::Verify, 0, Bytes::new()));
        assert_eq!(FrameValidationPolicy::new(&t, 1), Err("verify frame after validation prefix"));
    }

    #[test]
    fn rejects_invalid_shape_and_budgets_without_overflow() {
        let s = sender();
        for bad in [
            vec![frame(FrameMode::Verify, 7, s.clone())],
            vec![frame(FrameMode::Verify, 3, Bytes::from(vec![1; 19]))],
            vec![frame(FrameMode::Verify, 3, Bytes::from(vec![0x22; 20]))],
        ] {
            assert!(FrameValidationPolicy::new(&tx(bad), 1).is_err());
        }
        assert!(FrameValidationPolicy::new(
            &tx(vec![
                frame(FrameMode::Verify, 3, s.clone()),
                frame(FrameMode::Default, 0, Bytes::new()),
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
}
