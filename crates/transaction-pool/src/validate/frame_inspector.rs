//! Public frame-prefix trace restrictions. This inspector never warms EVM state.

use super::{frame_policy::FrameValidationPolicy, frame_state::FrameDependencies};
use alloy_eips::{
    eip8141::{Frame, FrameReceipt, FrameStatus, EXPIRY_VERIFIER, EXPIRY_VERIFIER_RUNTIME},
    eip8272::{RECENT_ROOT_ADDRESS, RECENT_ROOT_CODE},
};
use alloy_primitives::{Address, Bytes, KECCAK256_EMPTY, U256};
use evm2::{
    evm::AccountInfo,
    interpreter::{op, opcode::OpCode, GasTracker, InstrStop, Interpreter, Message, MessageResult},
    DatabaseError, Evm, EvmTypes, Inspector,
};
use std::collections::BTreeSet;

/// Enforces public validation-prefix policy independently of consensus execution.
///
/// The first violation remains fatal even when its call is reverted or caught. Callers must
/// check [`Self::database_error`] and [`Self::error`] before accepting the prefix result.
#[derive(Debug)]
pub struct FrameValidationInspector {
    sender: Address,
    policy: FrameValidationPolicy,
    current_frame: Option<usize>,
    error: Option<&'static str>,
    database_error: Option<DatabaseError>,
    accounts: BTreeSet<Address>,
    code: BTreeSet<Address>,
    mutable_code: BTreeSet<Address>,
    storage: BTreeSet<(Address, U256)>,
    expiry: Option<u64>,
}

impl FrameValidationInspector {
    /// Creates a fresh inspector for one structurally checked transaction.
    pub const fn new(sender: Address, policy: FrameValidationPolicy) -> Self {
        Self {
            sender,
            policy,
            current_frame: None,
            error: None,
            database_error: None,
            accounts: BTreeSet::new(),
            code: BTreeSet::new(),
            mutable_code: BTreeSet::new(),
            storage: BTreeSet::new(),
            expiry: None,
        }
    }

    /// Returns the first policy violation, including violations in reverted child calls.
    pub const fn error(&self) -> Option<&'static str> {
        self.error
    }

    /// Returns a state-read failure, which takes precedence over policy or execution failures.
    pub const fn database_error(&self) -> Option<&DatabaseError> {
        self.database_error.as_ref()
    }

    /// Returns the canonical expiry deadline, if representable as a block timestamp.
    pub const fn expiry(&self) -> Option<u64> {
        self.expiry
    }

    /// Returns the account, code and storage dependencies observed while executing the prefix.
    pub fn dependencies(&self) -> FrameDependencies {
        FrameDependencies {
            accounts: self.accounts.iter().copied().chain([self.sender]).collect(),
            code: self.code.iter().copied().collect(),
            mutable_code: self.mutable_code.iter().copied().collect(),
            storage: self.storage.iter().copied().collect(),
        }
    }

    fn reject(&mut self, reason: &'static str) {
        self.error.get_or_insert(reason);
    }

    fn deploying(&self) -> bool {
        self.policy.deploy_index.is_some() && self.policy.deploy_index == self.current_frame
    }

    fn executing_recent_root_verifier<T: EvmTypes>(&self, interp: &Interpreter<'_, '_, T>) -> bool {
        let message = interp.message();
        message.depth == 0 &&
            self.policy.recent_root.as_ref().is_some_and(|verifier| {
                Some(verifier.index) == self.current_frame &&
                    message.destination == RECENT_ROOT_ADDRESS &&
                    message.code_address == RECENT_ROOT_ADDRESS &&
                    interp.original_bytecode_slice() == RECENT_ROOT_CODE.as_ref()
            })
    }

    fn is_recent_root_storage_slot(&self, slot: U256) -> bool {
        self.policy.recent_root.as_ref().is_some_and(|verifier| {
            verifier.references.iter().any(|reference| reference.dependency().storage_key == slot)
        })
    }

    fn account<T: EvmTypes>(
        host: &mut Evm<'_, T>,
        address: Address,
    ) -> Result<AccountInfo, DatabaseError> {
        host.state_mut().account_info_untracked(&address).map(Option::unwrap_or_default)
    }

    fn check_code<T: EvmTypes>(
        &mut self,
        host: &mut Evm<'_, T>,
        address: Address,
        allow_empty: bool,
        sender_entry: bool,
    ) -> bool {
        self.code.insert(address);
        if host.precompiles().contains(&address) {
            return true
        }
        let info = match Self::account(host, address) {
            Ok(info) => info,
            Err(error) => {
                self.database_error.get_or_insert(error);
                return false
            }
        };
        if info.code_hash == KECCAK256_EMPTY || info.code_hash.is_zero() {
            if !allow_empty {
                self.reject("validation accessed an account without code");
            }
            return allow_empty
        }
        let code = match info.code {
            Some(code) => code,
            None => match host.database_mut().get_code_by_hash(&info.code_hash) {
                Ok(code) => code,
                Err(error) => {
                    self.database_error.get_or_insert(error);
                    return false
                }
            },
        };
        if let Some(delegate) = code.eip7702_address() &&
            !(sender_entry &&
                address == self.sender &&
                self.check_code(host, delegate, false, false))
        {
            self.reject("validation accessed delegated code");
            return false
        }
        if !(sender_entry && address == self.sender) {
            let mut instructions = code.original_byte_slice();
            while let Some((&opcode, rest)) = instructions.split_first() {
                if opcode == op::SETCODEFROM {
                    self.reject("validation accessed mutable code");
                    return false
                }
                if matches!(opcode, op::DELEGATECALL | op::CALLCODE) {
                    self.mutable_code.insert(address);
                }
                instructions = rest
                    .get(usize::from(OpCode::new_or_unknown(opcode).immediate_size())..)
                    .unwrap_or_default();
            }
        }
        true
    }

    fn rejected<T: EvmTypes>(&self, message: &Message<T>) -> Option<MessageResult<T>> {
        (self.error.is_some() || self.database_error.is_some()).then(|| {
            let mut gas = GasTracker::new_isolated(message.gas_limit, message.reservoir);
            gas.spend_all();
            MessageResult::<T> { stop: InstrStop::OutOfGas, gas, ..Default::default() }
        })
    }
}

impl<T: EvmTypes> Inspector<T> for FrameValidationInspector {
    fn frame_start(&mut self, index: usize, frame: &Frame, _: &mut Evm<'_, T>) {
        self.current_frame = Some(index);
        self.code.insert(frame.target_address().unwrap_or(self.sender));
        if index >= self.policy.prefix_end {
            self.reject("execution escaped validation prefix");
        }
    }

    fn frame_end(
        &mut self,
        _: usize,
        receipt: &FrameReceipt,
        _: &Bytes,
        _: InstrStop,
        host: &mut Evm<'_, T>,
    ) {
        if receipt.status != FrameStatus::Success {
            self.reject("validation prefix frame failed");
        }
        if self.deploying() {
            match Self::account(host, self.sender) {
                Ok(info) if info.code_hash != KECCAK256_EMPTY && !info.code_hash.is_zero() => {}
                Ok(_) => self.reject("deploy frame did not install sender code"),
                Err(error) => {
                    self.database_error.get_or_insert(error);
                }
            }
        }
        self.current_frame = None;
    }

    fn step(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        let opcode = interp.opcode();
        let deploying = self.deploying();
        let address = interp.message().destination;
        let recent_root = self.executing_recent_root_verifier(interp);
        let expiry_timestamp = opcode == 0x42 &&
            self.policy.expiry_index.is_some() &&
            self.policy.expiry_index == self.current_frame &&
            interp.message().code_address == EXPIRY_VERIFIER &&
            interp.original_bytecode_slice() == EXPIRY_VERIFIER_RUNTIME;

        match opcode {
            0x31 |
            0x3a |
            0x40 |
            0x41 |
            0x43..=0x45 |
            0x47 |
            0x48 |
            0x4a |
            op::SETCODEFROM |
            0xfe |
            0xff => self.reject("banned opcode in validation prefix"),
            0x42 if !expiry_timestamp => self.reject("TIMESTAMP outside canonical expiry verifier"),
            0x4b if !recent_root => self.reject("SLOTNUM outside canonical recent root verifier"),
            0x5a if !matches!(
                interp.original_bytecode_slice().get(interp.pc() + 1),
                Some(0xf1 | 0xf2 | 0xf4 | 0xfa)
            ) =>
            {
                self.reject("GAS must immediately precede a call")
            }
            0xf0 | 0xf5 | 0xf6 if !deploying => {
                self.reject("code installation outside deploy frame")
            }
            0x54 | 0x55 => {
                if recent_root {
                    if opcode == 0x55 {
                        self.reject("recent root verifier attempted a storage write")
                    } else if let Some(slot) = interp.stack().peek(0) {
                        if self.is_recent_root_storage_slot(slot) {
                            self.storage.insert((RECENT_ROOT_ADDRESS, slot));
                        } else {
                            self.reject("recent root verifier accessed an undeclared storage slot")
                        }
                    }
                } else if address != self.sender {
                    self.reject("validation accessed storage outside sender")
                } else if opcode == 0x55 && !deploying {
                    self.reject("storage write outside deploy frame")
                } else if let Some(slot) = interp.stack().peek(0) {
                    self.storage.insert((address, slot));
                }
            }
            0x3b | 0x3c | 0x3f => {
                if let Some(word) = interp.stack().peek(0) {
                    let address = Address::from_word(word.into());
                    self.check_code(interp.host(), address, address == self.sender, false);
                }
            }
            _ => {}
        }
        if self.error.is_some() || self.database_error.is_some() {
            interp.set_stop(InstrStop::Revert);
        }
    }

    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        message: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        let top_level = message.depth == 0;
        if self.current_frame.is_none_or(|index| index >= self.policy.prefix_end) {
            self.reject("execution escaped validation prefix");
        }
        if !message.value.is_zero() {
            self.reject("value transfer in validation prefix");
        }
        self.check_code(
            interp.host(),
            message.call_target,
            top_level || message.call_target == self.sender,
            top_level,
        );
        if top_level &&
            self.current_frame.is_some() &&
            self.current_frame == self.policy.expiry_index
        {
            if message.call_target != EXPIRY_VERIFIER ||
                message.code.original_byte_slice() != EXPIRY_VERIFIER_RUNTIME
            {
                self.reject("expiry verifier runtime does not match canonical code");
            } else {
                match <[u8; 8]>::try_from(message.input.as_ref()) {
                    Ok(deadline) => self.expiry = Some(u64::from_be_bytes(deadline)),
                    Err(_) => self.reject("invalid expiry deadline encoding"),
                }
            }
        }
        if top_level &&
            self.current_frame == self.policy.recent_root.as_ref().map(|verifier| verifier.index) &&
            (message.call_target != RECENT_ROOT_ADDRESS ||
                message.code.original_byte_slice() != RECENT_ROOT_CODE.as_ref())
        {
            self.reject("recent root verifier runtime does not match canonical code");
        }
        self.rejected::<T>(message)
    }

    fn call_end(
        &mut self,
        _: &mut Interpreter<'_, '_, T>,
        message: &Message<T>,
        result: &mut MessageResult<T>,
    ) {
        if message.depth == 0 && !result.stop.is_success() {
            self.reject("validation prefix frame failed");
        }
    }

    fn create(
        &mut self,
        _: &mut Interpreter<'_, '_, T>,
        message: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        self.accounts.insert(message.caller);
        if !self.deploying() {
            self.reject("creation outside deploy frame");
        }
        if message.destination != self.sender {
            self.reject("deploy frame created an account other than sender");
        }
        self.rejected::<T>(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{transaction::Recovered, TxEip8141};
    use alloy_eips::eip8141::{FrameAddress, FrameLimits, FrameMode};
    use alloy_primitives::B256;
    use evm2::{
        bytecode::Bytecode,
        env::BlockEnvExt,
        ethereum::{ethereum_tx_registry, TxEnvelope},
        evm::{Database, Db, InMemoryDB},
        interpreter::MessageKind,
        registry::{FrameTransactionError, HandlerError},
        BaseEvmTypes, Precompiles, SpecId,
    };

    #[derive(Debug)]
    struct FailingAccountDatabase;

    impl Database for FailingAccountDatabase {
        type Error = std::io::Error;

        fn get_account(&mut self, _: &Address) -> Result<Option<AccountInfo>, Self::Error> {
            Err(std::io::Error::other("frame account read failed"))
        }

        fn get_code_by_hash(&mut self, _: &B256) -> Result<Bytecode, Self::Error> {
            Ok(Bytecode::default())
        }

        fn get_storage(&mut self, _: &Address, _: &U256) -> Result<U256, Self::Error> {
            Ok(U256::ZERO)
        }

        fn get_block_hash(&mut self, _: &U256) -> Result<B256, Self::Error> {
            Ok(B256::ZERO)
        }
    }

    fn policy() -> FrameValidationPolicy {
        FrameValidationPolicy {
            prefix_end: 1,
            deploy_index: None,
            expiry_index: None,
            recent_root: None,
            declared_execution_gas: 10_000,
            state_gas: 0,
        }
    }

    #[test]
    fn dependency_database_failure_preserves_source() {
        let sender = Address::repeat_byte(1);
        let mut evm = Evm::<BaseEvmTypes>::new(
            SpecId::BOGOTA,
            BlockEnvExt::default(),
            ethereum_tx_registry(SpecId::BOGOTA),
            Db::new(FailingAccountDatabase),
            Precompiles::base(SpecId::BOGOTA),
        );
        let mut inspector = FrameValidationInspector::new(sender, policy());
        assert!(!inspector.check_code(&mut evm, sender, true, true));
        assert_eq!(inspector.error(), None);
        let error = inspector.database_error().unwrap();
        assert!(error.is_fatal());
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().to_string(),
            "frame account read failed"
        );
        assert!(inspector.rejected::<BaseEvmTypes>(&Message::<BaseEvmTypes>::default()).is_some());
    }

    #[test]
    fn rejects_environment_reads_and_requires_immediate_call_after_gas() {
        for opcode in [
            0x31,
            0x3a,
            0x40,
            0x41,
            0x42,
            0x43,
            0x44,
            0x45,
            0x47,
            0x48,
            0x4a,
            0x4b,
            op::SETCODEFROM,
            0xfe,
            0xff,
        ] {
            let mut inspector = FrameValidationInspector::new(Address::ZERO, policy());
            let message = Message::<BaseEvmTypes> {
                code: Bytecode::new_legacy(Bytes::from(vec![opcode])),
                ..Default::default()
            };
            let mut interp = Interpreter::<BaseEvmTypes>::new(&message);
            inspector.step(&mut interp);
            let reason = match opcode {
                0x42 => "TIMESTAMP outside canonical expiry verifier",
                0x4b => "SLOTNUM outside canonical recent root verifier",
                _ => "banned opcode in validation prefix",
            };
            assert_eq!(inspector.error(), Some(reason), "opcode {opcode:#x}");
            assert_eq!(interp.result(), Err(InstrStop::Revert));
        }
        for next in [0xf1, 0xf2, 0xf4, 0xfa, 0x00, 0x50] {
            let mut inspector = FrameValidationInspector::new(Address::ZERO, policy());
            let message = Message::<BaseEvmTypes> {
                code: Bytecode::new_legacy(Bytes::from(vec![0x5a, next])),
                ..Default::default()
            };
            let mut interp = Interpreter::<BaseEvmTypes>::new(&message);
            inspector.step(&mut interp);
            assert_eq!(
                inspector.error(),
                (!matches!(next, 0xf1 | 0xf2 | 0xf4 | 0xfa))
                    .then_some("GAS must immediately precede a call")
            );
        }
    }

    #[test]
    fn factory_nonce_is_a_validation_dependency() {
        let factory = Address::repeat_byte(2);
        let sender = factory.create(0);
        let mut policy = policy();
        policy.deploy_index = Some(0);
        let mut inspector = FrameValidationInspector::new(sender, policy);
        inspector.current_frame = Some(0);
        let mut message = Message::<BaseEvmTypes> {
            kind: MessageKind::Create,
            caller: factory,
            destination: sender,
            ..Default::default()
        };
        let active_message = message.clone();
        let mut interp = Interpreter::<BaseEvmTypes>::new(&active_message);
        assert!(inspector.create(&mut interp, &mut message).is_none());
        assert_eq!(inspector.error(), None);
        assert_eq!(inspector.dependencies().accounts, vec![factory, sender]);
    }

    #[test]
    fn transient_storage_is_not_a_persistent_state_dependency() {
        let sender = Address::repeat_byte(1);
        for address in [sender, Address::repeat_byte(2)] {
            for opcode in [0x5c, 0x5d] {
                let mut inspector = FrameValidationInspector::new(sender, policy());
                let message = Message::<BaseEvmTypes> {
                    destination: address,
                    code: Bytecode::new_legacy(Bytes::from(vec![opcode])),
                    ..Default::default()
                };
                let mut interp = Interpreter::<BaseEvmTypes>::new(&message);
                interp.stack_mut().push(U256::from(7)).unwrap();
                inspector.step(&mut interp);
                assert_eq!(inspector.error(), None);
                assert_eq!(inspector.dependencies().storage, vec![]);
            }
        }
    }

    #[test]
    fn sender_storage_only_and_writes_only_during_deploy() {
        let sender = Address::repeat_byte(1);
        for (address, opcode, deploy, allowed) in [
            (sender, 0x54, false, true),
            (Address::ZERO, 0x54, false, false),
            (sender, 0x55, false, false),
            (sender, 0x55, true, true),
            (Address::ZERO, 0x55, true, false),
        ] {
            let mut policy = policy();
            policy.deploy_index = deploy.then_some(0);
            let mut inspector = FrameValidationInspector::new(sender, policy);
            inspector.current_frame = Some(0);
            let message = Message::<BaseEvmTypes> {
                destination: address,
                code: Bytecode::new_legacy(Bytes::from(vec![opcode])),
                ..Default::default()
            };
            let mut interp = Interpreter::<BaseEvmTypes>::new(&message);
            interp.stack_mut().push(U256::from(7)).unwrap();
            inspector.step(&mut interp);
            let expected = if allowed {
                None
            } else if address != sender {
                Some("validation accessed storage outside sender")
            } else {
                Some("storage write outside deploy frame")
            };
            assert_eq!(inspector.error(), expected);
            if allowed {
                assert_eq!(inspector.dependencies().storage, vec![(sender, U256::from(7))]);
            }
        }
    }

    #[test]
    fn inspected_prefix_reverts_runtime_and_skips_suffix() {
        let sender = Address::repeat_byte(1);
        let suffix = Address::repeat_byte(2);
        for (code, expected_error) in [
            (&[0x60, 3, 0x5f, 0x5f, 0xaa, 0x00][..], None),
            (&[0x5f, 0x5c, 0x50, 0x60, 3, 0x5f, 0x5f, 0xaa, 0x00][..], None),
            (
                &[0x5f, 0x5f, 0x5d, 0x60, 3, 0x5f, 0x5f, 0xaa, 0x00][..],
                Some("validation prefix frame failed"),
            ),
            (
                &[0x42, 0x60, 3, 0x5f, 0x5f, 0xaa, 0x00][..],
                Some("TIMESTAMP outside canonical expiry verifier"),
            ),
        ] {
            let payload = TxEip8141 {
                chain_id: 1,
                sender,
                nonce_keys: Some(vec![]),
                frames: vec![
                    Frame {
                        mode: FrameMode::Verify,
                        flags: 3,
                        limits: FrameLimits { execution: 10_000, state: 0 },
                        ..Default::default()
                    },
                    Frame {
                        mode: FrameMode::Sender,
                        target: FrameAddress::from(suffix),
                        limits: FrameLimits { execution: 10_000, state: 0 },
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            let policy =
                FrameValidationPolicy::new(&payload, payload.signature_verification_gas()).unwrap();
            let prefix_end = policy.prefix_end;
            let transaction = Recovered::new_unchecked(TxEnvelope::Eip8141(payload), sender);
            let mut db = InMemoryDB::default();
            db.insert_account_info(
                &sender,
                AccountInfo::default()
                    .with_code(Bytecode::new_legacy(Bytes::copy_from_slice(code))),
            );
            db.insert_account_info(
                &suffix,
                AccountInfo::default().with_code(Bytecode::new_legacy(Bytes::from_static(&[0xfe]))),
            );
            let mut evm = Evm::<BaseEvmTypes>::new(
                SpecId::BOGOTA,
                BlockEnvExt::default(),
                ethereum_tx_registry(SpecId::BOGOTA),
                db,
                Precompiles::base(SpecId::BOGOTA),
            );
            evm.set_inspector(FrameValidationInspector::new(sender, policy));
            let result = evm.validate_frame_prefix(&transaction, prefix_end);
            if expected_error.is_none() {
                assert!(result.unwrap().sender_approved);
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    HandlerError::InvalidFrameTransaction(FrameTransactionError::VerifyFrameFailed)
                );
            }
            let inspector = evm.clear_inspector_as::<FrameValidationInspector>().unwrap();
            assert_eq!(inspector.error(), expected_error);
            assert_eq!(evm.state_mut().account(&sender).unwrap().nonce(), 0);
            assert_eq!(inspector.dependencies().code, vec![sender]);
        }
    }

    #[test]
    fn mutable_code_scan_skips_immediates_and_allows_only_sender_entry() {
        let sender = Address::repeat_byte(1);
        let helper = Address::repeat_byte(2);
        for (bytes, mutable) in [
            (vec![op::SETCODEFROM], true),
            (vec![op::STOP, op::SETCODEFROM], true),
            (vec![op::PUSH1, op::SETCODEFROM, op::STOP], false),
            (vec![op::PUSH32, op::SETCODEFROM], false),
            (vec![op::PUSH1, 0, op::SETCODEFROM], true),
        ] {
            for (address, entry) in
                [(sender, true), (sender, false), (helper, true), (helper, false)]
            {
                let mut db = InMemoryDB::default();
                db.insert_account_info(
                    &address,
                    AccountInfo::default().with_code(Bytecode::new_legacy(bytes.clone().into())),
                );
                let mut evm = Evm::<BaseEvmTypes>::new(
                    SpecId::BOGOTA,
                    BlockEnvExt::default(),
                    ethereum_tx_registry(SpecId::BOGOTA),
                    db,
                    Precompiles::base(SpecId::BOGOTA),
                );
                let mut inspector = FrameValidationInspector::new(sender, policy());
                let rejected = mutable && !(address == sender && entry);
                assert_eq!(inspector.check_code(&mut evm, address, false, entry), !rejected);
                assert_eq!(
                    inspector.error(),
                    rejected.then_some("validation accessed mutable code")
                );
                assert_eq!(inspector.dependencies().code, vec![address]);
            }
        }
    }

    #[test]
    fn delegated_sender_cannot_hide_mutable_target_code() {
        let sender = Address::repeat_byte(1);
        let helper = Address::repeat_byte(2);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            &sender,
            AccountInfo::default().with_code(Bytecode::new_eip7702(helper)),
        );
        db.insert_account_info(
            &helper,
            AccountInfo::default()
                .with_code(Bytecode::new_legacy(Bytes::from_static(&[op::SETCODEFROM]))),
        );
        let mut evm = Evm::<BaseEvmTypes>::new(
            SpecId::BOGOTA,
            BlockEnvExt::default(),
            ethereum_tx_registry(SpecId::BOGOTA),
            db,
            Precompiles::base(SpecId::BOGOTA),
        );
        let mut inspector = FrameValidationInspector::new(sender, policy());
        assert!(!inspector.check_code(&mut evm, sender, false, true));
        assert_eq!(inspector.error(), Some("validation accessed mutable code"));
        assert_eq!(inspector.dependencies().code, vec![sender, helper]);
    }

    #[test]
    fn delegated_execution_code_is_a_bounded_dependency() {
        let sender = Address::repeat_byte(1);
        let helper = Address::repeat_byte(2);
        for opcode in [op::DELEGATECALL, op::CALLCODE] {
            for (code, mutable) in [
                (vec![op::STOP, opcode], true),
                (vec![op::PUSH1, opcode, op::STOP], false),
                (vec![op::PUSH32, opcode], false),
            ] {
                let mut db = InMemoryDB::default();
                db.insert_account_info(
                    &helper,
                    AccountInfo::default().with_code(Bytecode::new_legacy(code.into())),
                );
                let mut evm = Evm::<BaseEvmTypes>::new(
                    SpecId::BOGOTA,
                    BlockEnvExt::default(),
                    ethereum_tx_registry(SpecId::BOGOTA),
                    db,
                    Precompiles::base(SpecId::BOGOTA),
                );
                let mut inspector = FrameValidationInspector::new(sender, policy());
                for _ in 0..2 {
                    assert!(inspector.check_code(&mut evm, helper, false, false));
                }
                assert_eq!(inspector.dependencies().code, vec![helper]);
                assert_eq!(
                    inspector.dependencies().mutable_code,
                    if mutable { vec![helper] } else { vec![] }
                );
            }
        }
    }
}
