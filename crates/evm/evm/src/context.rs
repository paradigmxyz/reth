//! EVM context traits and validation errors.

pub use revm::{
    context::{BlockEnv, Context, DBErrorMarker, TxEnv},
    context_interface::{
        result::{EVMError, HaltReason, InvalidTransaction, ResultAndState},
        Block, Cfg, ContextTr,
    },
    MainBuilder, MainContext,
};
