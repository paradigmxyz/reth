//! Providers implementations fitted to Scroll needs.

mod engine;
pub use engine::{
    ScrollAuthApiEngineClient, ScrollAuthEngineApiProvider, ScrollEngineApi, ScrollEngineApiResult,
};

mod error;
pub use error::ScrollEngineApiError;
