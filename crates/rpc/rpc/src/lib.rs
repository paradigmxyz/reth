#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth RPC implementation
//!
//! Provides the implementation of all RPC interfaces.
//!
//!
//! ## Note on blocking behaviour
//!
//! All async RPC handlers must non-blocking, see also [What is blocking](https://ryhl.io/blog/async-what-is-blocking/).
//!
//! A lot of the RPC are using a mix of async and direct calls to the database, which are blocking
//! and can reduce overall performance of all concurrent requests handled via the jsonrpsee server.
//!
//! To avoid this, all blocking or CPU intensive handlers must be spawned to a separate task. See
//! the [EthApi] handler implementations for examples. The rpc-api traits make no use of the
//! available jsonrpsee `blocking` attribute to give implementors more freedom because the
//! `blocking` attribute and async handlers are mutually exclusive. However, as mentioned above, a
//! lot of handlers make use of async functions, caching for example, but are also using blocking
//! disk-io, hence these calls are spawned as futures to a blocking task manually.

mod admin;
mod call_guard;
mod debug;
mod engine;
pub mod eth;
mod layers;
mod net;
mod trace;
mod txpool;
mod web3;

pub use admin::AdminApi;
pub use call_guard::TracingCallGuard;
pub use debug::DebugApi;
pub use engine::{EngineApi, EngineEthApi};
pub use eth::{EthApi, EthApiSpec, EthFilter, EthPubSub, EthSubscriptionIdProvider};
pub use layers::{AuthLayer, AuthValidator, Claims, JwtAuthValidator, JwtError, JwtSecret};
pub use net::NetApi;
pub use trace::TraceApi;
pub use txpool::TxPoolApi;
pub use web3::Web3Api;

pub(crate) mod result;
