//! EVM Handler related to Bsc chain

use super::{spec::BscSpecId, transaction::BscTxTr};
use alloy_primitives::{address, Address, U256};
use revm::{
    context::{
        result::{ExecutionResult, HaltReason},
        Cfg, ContextTr, JournalOutput, Transaction,
    },
    context_interface::{result::ResultAndState, JournalTr},
    handler::{handler::EvmTrError, EvmTr, Frame, FrameResult, Handler, MainnetHandler},
    inspector::{Inspector, InspectorEvmTr, InspectorFrame, InspectorHandler},
    interpreter::{
        interpreter::EthInterpreter, FrameInput, Host, InitialAndFloorGas, SuccessOrHalt,
    },
};

const SYSTEM_ADDRESS: Address = address!("fffffffffffffffffffffffffffffffffffffffe");

pub struct BscHandler<EVM, ERROR, FRAME> {
    pub mainnet: MainnetHandler<EVM, ERROR, FRAME>,
}

impl<EVM, ERROR, FRAME> BscHandler<EVM, ERROR, FRAME> {
    pub fn new() -> Self {
        Self { mainnet: MainnetHandler::default() }
    }
}

impl<EVM, ERROR, FRAME> Default for BscHandler<EVM, ERROR, FRAME> {
    fn default() -> Self {
        Self::new()
    }
}

pub trait BscContextTr:
    ContextTr<Journal: JournalTr<FinalOutput = JournalOutput>, Tx: BscTxTr, Cfg: Cfg<Spec = BscSpecId>>
{
}

impl<T> BscContextTr for T where
    T: ContextTr<
        Journal: JournalTr<FinalOutput = JournalOutput>,
        Tx: BscTxTr,
        Cfg: Cfg<Spec = BscSpecId>,
    >
{
}

impl<EVM, ERROR, FRAME> Handler for BscHandler<EVM, ERROR, FRAME>
where
    EVM: EvmTr<Context: BscContextTr>,
    ERROR: EvmTrError<EVM>,
    FRAME: Frame<Evm = EVM, Error = ERROR, FrameResult = FrameResult, FrameInit = FrameInput>,
{
    type Evm = EVM;
    type Error = ERROR;
    type Frame = FRAME;
    type HaltReason = HaltReason;

    fn validate_initial_tx_gas(
        &self,
        evm: &Self::Evm,
    ) -> Result<revm::interpreter::InitialAndFloorGas, Self::Error> {
        let ctx = evm.ctx_ref();
        let tx = ctx.tx();

        if tx.is_system_transaction() {
            return Ok(InitialAndFloorGas { initial_gas: 0, floor_gas: 0 });
        }

        self.mainnet.validate_initial_tx_gas(evm)
    }

    fn reward_beneficiary(
        &self,
        evm: &mut Self::Evm,
        exec_result: &mut <Self::Frame as Frame>::FrameResult,
    ) -> Result<(), Self::Error> {
        let ctx = evm.ctx();
        let tx = ctx.tx();

        if tx.is_system_transaction() {
            return Ok(());
        }

        let effective_gas_price = ctx.effective_gas_price();
        let gas = exec_result.gas();
        let mut tx_fee = U256::from(gas.spent() - gas.refunded() as u64) * effective_gas_price;

        // EIP-4844
        let is_cancun = ctx.cfg().spec().is_enabled_in(BscSpecId::CANCUN);
        if is_cancun {
            let data_fee = tx.calc_max_data_fee();
            tx_fee = tx_fee.saturating_add(data_fee);
        }

        let system_account = ctx.journal().load_account(SYSTEM_ADDRESS)?;
        system_account.data.mark_touch();
        system_account.data.info.balance = system_account.data.info.balance.saturating_add(tx_fee);
        Ok(())
    }

    fn output(
        &self,
        evm: &mut Self::Evm,
        result: <Self::Frame as Frame>::FrameResult,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        let ctx = evm.ctx();
        ctx.error();
        let tx = ctx.tx();
        // used gas with refund calculated.
        let gas_refunded =
            if tx.is_system_transaction() { 0 } else { result.gas().refunded() as u64 };
        let final_gas_used = result.gas().spent() - gas_refunded;
        let output = result.output();
        let instruction_result = result.into_interpreter_result();

        // reset journal and return present state.
        let final_state = ctx.journal().finalize();
        let logs = final_state.logs;
        let state = final_state.state;

        let result = match instruction_result.result.into() {
            SuccessOrHalt::Success(reason) => ExecutionResult::Success {
                reason,
                gas_used: final_gas_used,
                gas_refunded,
                logs,
                output,
            },
            SuccessOrHalt::Revert => {
                ExecutionResult::Revert { gas_used: final_gas_used, output: output.into_data() }
            }
            SuccessOrHalt::Halt(reason) => {
                ExecutionResult::Halt { reason, gas_used: final_gas_used }
            }
            // Only two internal return flags.
            flag @ (SuccessOrHalt::FatalExternalError | SuccessOrHalt::Internal(_)) => {
                panic!(
                "Encountered unexpected internal return flag: {flag:?} with instruction result: {instruction_result:?}"
            )
            }
        };

        Ok(ResultAndState { result, state })
    }
}

impl<EVM, ERROR, FRAME> InspectorHandler for BscHandler<EVM, ERROR, FRAME>
where
    EVM: InspectorEvmTr<
        Context: BscContextTr,
        Inspector: Inspector<<<Self as Handler>::Evm as EvmTr>::Context, EthInterpreter>,
    >,
    ERROR: EvmTrError<EVM>,
    FRAME: Frame<Evm = EVM, Error = ERROR, FrameResult = FrameResult, FrameInit = FrameInput>
        + InspectorFrame<IT = EthInterpreter>,
{
    type IT = EthInterpreter;
}
