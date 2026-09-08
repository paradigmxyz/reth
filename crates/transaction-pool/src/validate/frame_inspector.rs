//! Public frame-prefix trace restrictions. This inspector never warms EVM state.

use super::{frame_policy::FrameValidationPolicy, frame_state::FrameDependencies};
use alloy_eips::eip8141::{EXPIRY_VERIFIER, EXPIRY_VERIFIER_RUNTIME};
use alloy_primitives::{Address, U256};
use revm::{
    context_interface::{ContextTr, JournalTr, LocalContextTr},
    interpreter::{
        interpreter_types::{InputsTr, Jumps, LegacyBytecode},
        CallInputs, CallOutcome, CreateInputs, CreateOutcome, InstructionResult, Interpreter,
    },
    state::{AccountInfo, EvmState},
    Database, Inspector,
};
use std::collections::BTreeSet;

/// Enforces validation-prefix policy independently of consensus execution rules.
///
/// A recorded violation remains fatal even if a parent catches the failed child call.
/// The caller must check [`Self::error`] before accepting the prefix result.
#[derive(Debug)]
pub struct FrameValidationInspector {
    sender: Address,
    policy: FrameValidationPolicy,
    depth: usize,
    error: Option<&'static str>,
    accounts: BTreeSet<Address>,
    code: BTreeSet<Address>,
    storage: BTreeSet<(Address, U256)>,
    expiry: Option<u64>,
}

impl FrameValidationInspector {
    /// Creates a fresh inspector for one structurally checked transaction.
    pub const fn new(sender: Address, policy: FrameValidationPolicy) -> Self {
        Self {
            sender,
            policy,
            depth: 0,
            error: None,
            accounts: BTreeSet::new(),
            code: BTreeSet::new(),
            storage: BTreeSet::new(),
            expiry: None,
        }
    }

    /// Returns the first policy violation, including violations in reverted child calls.
    pub const fn error(&self) -> Option<&'static str> {
        self.error
    }

    /// Returns the canonical expiry deadline, if representable as a block timestamp.
    pub const fn expiry(&self) -> Option<u64> {
        self.expiry
    }

    /// Returns the state dependencies observed during prefix execution.
    pub fn dependencies(&self) -> FrameDependencies {
        FrameDependencies {
            accounts: self.accounts.iter().copied().chain([self.sender]).collect(),
            code: self.code.iter().copied().collect(),
            storage: self.storage.iter().copied().collect(),
        }
    }

    fn reject(&mut self, reason: &'static str) {
        self.error.get_or_insert(reason);
    }

    fn frame_index<CTX: ContextTr>(ctx: &CTX) -> Option<usize> {
        ctx.local().frame_transaction().map(|runtime| runtime.current_frame_index)
    }

    fn deploying<CTX: ContextTr>(&self, ctx: &CTX) -> bool {
        self.policy.deploy_index.is_some() && self.policy.deploy_index == Self::frame_index(ctx)
    }

    /// Read through the journal without changing warm/cold access accounting.
    fn account<CTX>(ctx: &mut CTX, address: Address) -> Result<AccountInfo, &'static str>
    where
        CTX: ContextTr,
        CTX::Journal: JournalTr<State = EvmState>,
    {
        let (db, state) = ctx.journal_mut().db_and_state_mut();
        if let Some(account) = state.get(&address) {
            return Ok(account.info.clone())
        }
        db.basic(address)
            .map(|info| info.unwrap_or_default())
            .map_err(|_| "frame dependency read failed")
    }

    fn check_code<CTX>(
        &mut self,
        ctx: &mut CTX,
        address: Address,
        allow_empty: bool,
        sender_entry: bool,
    ) -> bool
    where
        CTX: ContextTr,
        CTX::Journal: JournalTr<State = EvmState>,
    {
        self.code.insert(address);
        if ctx.journal().precompile_addresses().contains(&address) {
            return true
        }
        let result = Self::account(ctx, address).and_then(|info| {
            if info.is_empty_code_hash() {
                return if allow_empty {
                    Ok(())
                } else {
                    Err("validation accessed an account without code")
                }
            }
            let code = match info.code {
                Some(code) => code,
                None => ctx
                    .db_mut()
                    .code_by_hash(info.code_hash)
                    .map_err(|_| "frame code read failed")?,
            };
            if let Some(delegate) = code.eip7702_address() {
                // Top-level sender verification can execute code installed by a deploy frame.
                // CALL*/EXTCODE* may not follow arbitrary third-party delegations.
                if sender_entry &&
                    address == self.sender &&
                    self.check_code(ctx, delegate, false, false)
                {
                    Ok(())
                } else {
                    Err("validation accessed delegated code")
                }
            } else {
                Ok(())
            }
        });
        if let Err(reason) = result {
            self.reject(reason);
            return false
        }
        true
    }
}

impl<CTX> Inspector<CTX> for FrameValidationInspector
where
    CTX: ContextTr,
    CTX::Journal: JournalTr<State = EvmState>,
{
    fn step(&mut self, interp: &mut Interpreter, ctx: &mut CTX) {
        let opcode = interp.bytecode.opcode();
        let deploying = self.deploying(ctx);
        let address = interp.input.target_address();
        let expiry_timestamp = opcode == 0x42 &&
            self.policy.expiry_index.is_some() &&
            self.policy.expiry_index == Self::frame_index(ctx) &&
            interp.input.bytecode_address() == Some(&EXPIRY_VERIFIER) &&
            interp.bytecode.bytecode_slice() == EXPIRY_VERIFIER_RUNTIME;

        match opcode {
            // Environment-dependent opcodes, arbitrary balance reads and state destruction.
            0x31 | 0x3a | 0x40 | 0x41 | 0x43..=0x45 | 0x47 | 0x48 | 0x4a | 0x4b | 0xfe | 0xff => {
                self.reject("banned opcode in validation prefix")
            }
            0x42 if !expiry_timestamp => self.reject("TIMESTAMP outside canonical expiry verifier"),
            0x5a if !matches!(
                interp.bytecode.bytecode_slice().get(interp.bytecode.pc() + 1),
                Some(0xf1 | 0xf2 | 0xf4 | 0xfa)
            ) =>
            {
                self.reject("GAS must immediately precede a call")
            }
            0xf0 | 0xf5 | 0xf6 if !deploying => {
                self.reject("code installation outside deploy frame")
            }
            0x54 | 0x55 => {
                if address != self.sender {
                    self.reject("validation accessed storage outside sender")
                } else if opcode == 0x55 && !deploying {
                    self.reject("storage write outside deploy frame")
                } else if let Some(slot) = interp.stack.data().last() {
                    self.storage.insert((address, *slot));
                }
            }
            0x3b | 0x3c | 0x3f => {
                if let Some(word) = interp.stack.data().last() {
                    let address = Address::from_word((*word).into());
                    self.check_code(ctx, address, address == self.sender, false);
                }
            }
            _ => {}
        }
        if self.error.is_some() {
            interp.halt(InstructionResult::Revert);
        }
    }

    fn call(&mut self, ctx: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let top_level = self.depth == 0;
        self.depth += 1;
        let index = Self::frame_index(ctx);
        if index.is_none_or(|index| index >= self.policy.prefix_end) {
            self.reject("execution escaped validation prefix");
        }
        if inputs.value.transfer().is_some_and(|value| !value.is_zero()) {
            self.reject("value transfer in validation prefix");
        }
        self.check_code(
            ctx,
            inputs.bytecode_address,
            top_level || inputs.bytecode_address == self.sender,
            top_level,
        );
        if top_level && index.is_some() && index == self.policy.expiry_index {
            if inputs.bytecode_address != EXPIRY_VERIFIER ||
                inputs.known_bytecode.1.original_byte_slice() != EXPIRY_VERIFIER_RUNTIME
            {
                self.reject("expiry verifier runtime does not match canonical code");
            } else {
                let data = inputs.input.as_bytes(ctx);
                match <[u8; 8]>::try_from(&*data) {
                    Ok(deadline) => self.expiry = Some(u64::from_be_bytes(deadline)),
                    Err(_) => self.reject("invalid expiry deadline encoding"),
                }
            }
        }
        self.error.map(|_| {
            CallOutcome::new_oog(
                inputs.gas_limit,
                inputs.return_memory_offset.clone(),
                inputs.reservoir,
            )
        })
    }

    fn call_end(&mut self, ctx: &mut CTX, _inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            if !outcome.result.result.is_ok() {
                self.reject("validation prefix frame failed");
            }
            if self.deploying(ctx) {
                match Self::account(ctx, self.sender) {
                    Ok(info) if !info.is_empty_code_hash() => {}
                    Ok(_) => self.reject("deploy frame did not install sender code"),
                    Err(reason) => self.reject(reason),
                }
            }
        }
    }

    fn create(&mut self, ctx: &mut CTX, inputs: &mut CreateInputs) -> Option<CreateOutcome> {
        self.depth += 1;
        // CREATE derives the sender address from the factory nonce. A nonce change must
        // invalidate the prefix even when the factory's code and storage are unchanged.
        self.accounts.insert(inputs.caller());
        if !self.deploying(ctx) {
            self.reject("creation outside deploy frame");
        }
        match Self::account(ctx, inputs.caller()) {
            Ok(info) if inputs.created_address(info.nonce) == self.sender => {}
            Ok(_) => self.reject("deploy frame created an account other than sender"),
            Err(reason) => self.reject(reason),
        }
        self.error.map(|_| CreateOutcome::new_oog(inputs.gas_limit(), inputs.reservoir()))
    }

    fn create_end(&mut self, _ctx: &mut CTX, _inputs: &CreateInputs, _outcome: &mut CreateOutcome) {
        self.depth = self.depth.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::TxEip8141;
    use alloy_eips::eip8141::{Frame, FrameLimits, FrameMode};
    use alloy_primitives::Bytes;
    use revm::{
        context::{transaction::FrameTransaction, Context, ContextSetters, TxEnv},
        context_interface::{local::FrameTransactionRuntime, ContextTr, JournalTr},
        database::{CacheDB, EmptyDB},
        handler::{MainBuilder, MainnetHandler},
        inspector::InspectorHandler,
        interpreter::interpreter::ExtBytecode,
        primitives::{hardfork::SpecId, TxKind},
        state::{AccountInfo, Bytecode},
        MainContext,
    };

    fn policy() -> FrameValidationPolicy {
        FrameValidationPolicy {
            prefix_end: 1,
            deploy_index: None,
            expiry_index: None,
            declared_execution_gas: 10_000,
            state_gas: 0,
        }
    }

    #[test]
    fn rejects_environment_reads_and_requires_immediate_call_after_gas() {
        for opcode in
            [0x31, 0x3a, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x47, 0x48, 0x4a, 0x4b, 0xfe, 0xff]
        {
            let mut inspector = FrameValidationInspector::new(Address::ZERO, policy());
            let mut ctx = Context::mainnet();
            let mut interp = Interpreter {
                bytecode: ExtBytecode::new(Bytecode::new_legacy(vec![opcode].into())),
                ..Default::default()
            };
            inspector.step(&mut interp, &mut ctx);
            assert!(inspector.error().is_some(), "opcode {opcode:#x}");
        }
        for next in [0xf1, 0xf2, 0xf4, 0xfa, 0x00, 0x50] {
            let mut inspector = FrameValidationInspector::new(Address::ZERO, policy());
            let mut ctx = Context::mainnet();
            let mut interp = Interpreter {
                bytecode: ExtBytecode::new(Bytecode::new_legacy(vec![0x5a, next].into())),
                ..Default::default()
            };
            inspector.step(&mut interp, &mut ctx);
            assert_eq!(inspector.error().is_none(), matches!(next, 0xf1 | 0xf2 | 0xf4 | 0xfa));
        }
    }

    #[test]
    fn factory_nonce_is_a_validation_dependency() {
        let factory = Address::repeat_byte(2);
        let sender = factory.create(0);
        let mut policy = policy();
        policy.deploy_index = Some(0);
        let mut inspector = FrameValidationInspector::new(sender, policy);
        let mut ctx = Context::mainnet();
        ctx.local_mut().set_frame_transaction(Some(FrameTransactionRuntime::new(sender)));
        let mut inputs = CreateInputs::new(
            factory,
            revm::context_interface::CreateScheme::Create,
            U256::ZERO,
            Bytes::new(),
            10_000,
            0,
        );
        assert!(inspector.create(&mut ctx, &mut inputs).is_none());
        assert!(inspector.error().is_none());
        assert!(inspector.dependencies().accounts.contains(&factory));
        assert!(ctx.journal().evm_state().is_empty());
    }

    #[test]
    fn transient_storage_is_not_a_persistent_state_dependency() {
        let sender = Address::repeat_byte(1);
        for address in [sender, Address::repeat_byte(2)] {
            for opcode in [0x5c, 0x5d] {
                let mut inspector = FrameValidationInspector::new(sender, policy());
                let mut ctx = Context::mainnet();
                let mut interp = Interpreter::default();
                interp.input.target_address = address;
                interp.bytecode = ExtBytecode::new(Bytecode::new_legacy(vec![opcode].into()));
                assert!(interp.stack.push(U256::from(7)));
                inspector.step(&mut interp, &mut ctx);
                // The EVM still enforces the static-context restriction on TSTORE.
                assert!(inspector.error().is_none());
                assert!(inspector.dependencies().storage.is_empty());
                assert!(ctx.journal().evm_state().is_empty());
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
            let mut ctx = Context::mainnet();
            ctx.local_mut().set_frame_transaction(Some(FrameTransactionRuntime::new(sender)));
            let mut interp = Interpreter::default();
            interp.input.target_address = address;
            interp.bytecode = ExtBytecode::new(Bytecode::new_legacy(vec![opcode].into()));
            assert!(interp.stack.push(U256::from(7)));
            inspector.step(&mut interp, &mut ctx);
            assert_eq!(inspector.error().is_none(), allowed);
            if allowed {
                assert_eq!(inspector.dependencies().storage, vec![(sender, U256::from(7))]);
            }
            // Inspecting does not itself insert/warm an account in the journal.
            assert!(ctx.journal().evm_state().is_empty());
        }
    }

    #[test]
    fn inspected_prefix_reverts_runtime_and_skips_suffix() {
        let sender = Address::repeat_byte(1);
        let suffix = Address::repeat_byte(2);

        let run = |sender_code: &[u8], succeeds: bool| {
            let frame_tx = TxEip8141 {
                sender,
                frames: vec![
                    Frame {
                        mode: FrameMode::Verify,
                        flags: 3,
                        limits: FrameLimits { execution: 10_000, state: 0 },
                        ..Default::default()
                    },
                    Frame {
                        mode: FrameMode::Sender,
                        target: Bytes::copy_from_slice(suffix.as_slice()),
                        limits: FrameLimits { execution: 10_000, state: 0 },
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            let policy =
                FrameValidationPolicy::new(&frame_tx, frame_tx.signature_verification_gas())
                    .unwrap();
            let payload = FrameTransaction {
                frames: frame_tx.frames.clone(),
                signatures: frame_tx.signatures.clone(),
                signature_hash: frame_tx.signature_hash(),
                max_priority_fee_per_gas: frame_tx.fees.max_priority_fee_per_gas,
                max_fee_per_gas: frame_tx.fees.max_fee_per_gas,
                max_fee_per_blob_gas: frame_tx.fees.max_fee_per_blob_gas,
            };
            let gas_limit = payload.gas_limit(sender).unwrap();
            let tx = TxEnv::builder()
                .tx_type(Some(0x06))
                .caller(sender)
                .kind(TxKind::Call(sender))
                .gas_limit(gas_limit)
                .gas_priority_fee(Some(0))
                .frame_transaction(payload)
                .build()
                .unwrap();

            let mut db = CacheDB::<EmptyDB>::default();
            db.insert_account_info(
                sender,
                AccountInfo::default().with_code(Bytecode::new_legacy(sender_code.to_vec().into())),
            );
            db.insert_account_info(
                suffix,
                AccountInfo::default().with_code(Bytecode::new_legacy(Bytes::from_static(&[0xfe]))),
            );
            let inspector = FrameValidationInspector::new(sender, policy);
            let mut evm = Context::mainnet()
                .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::BOGOTA))
                .with_db(db)
                .build_mainnet_with_inspector(inspector);
            evm.ctx.set_tx(tx);

            let mut handler: MainnetHandler<
                _,
                revm::context::result::EVMError<core::convert::Infallible>,
                _,
            > = MainnetHandler::default();
            let result = handler.inspect_validate_prefix(&mut evm, policy.prefix_end);
            assert_eq!(result.is_ok(), succeeds);
            let inspector = &evm.inspector;
            assert_eq!(inspector.error().is_none(), succeeds);
            assert!(evm.ctx.local().frame_transaction().is_none());
            assert_eq!(
                evm.ctx.journal().evm_state().get(&sender).map_or(0, |account| account.info.nonce),
                0
            );
            assert_eq!(inspector.dependencies().code, vec![sender]);
        };

        // APPROVE(3) validates the sender and payer. The suffix is INVALID and must not run.
        run(&[0x60, 0x03, 0x5f, 0x5f, 0xaa, 0x00], true);
        // TLOAD is valid in VERIFY; TSTORE still fails under EVM static-call rules.
        run(&[0x5f, 0x5c, 0x50, 0x60, 0x03, 0x5f, 0x5f, 0xaa, 0x00], true);
        run(&[0x5f, 0x5f, 0x5d, 0x60, 0x03, 0x5f, 0x5f, 0xaa, 0x00], false);
        // A rejected opcode remains sticky even though APPROVE would otherwise succeed.
        run(&[0x42, 0x60, 0x03, 0x5f, 0x5f, 0xaa, 0x00], false);
    }
}
