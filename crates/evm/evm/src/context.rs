//! EVM context traits and validation errors.

pub use revm::{
    context::{BlockEnv, Context, DBErrorMarker, TxEnv},
    context_interface::{
        result::{
            EVMError, ExecResultAndState, ExecutionResult, HaltReason, InvalidHeader,
            InvalidTransaction, OutOfGasError, Output, ResultAndState, ResultGas, SuccessReason,
        },
        Block, Cfg, ContextTr,
    },
    MainBuilder, MainContext,
};
