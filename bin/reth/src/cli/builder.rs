//! Support for integrating customizations into the CLI.
use clap::Args;

pub trait RethCliBuilder : Args + Sized {
    /// A type that knows how to create reth's RPC server.
    type Rpc : RethRpcServerBuilder;
}


/// A trait that allows customizing the RPC server creation
#[async_trait::async_trait]
pub trait RethRpcServerBuilder : Sized {

}