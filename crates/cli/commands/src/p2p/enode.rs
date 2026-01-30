//! Enode identifier command

use clap::Parser;
use reth_cli_util::get_secret_key;
use reth_network_peers::NodeRecord;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

/// Print the enode identifier for a given secret key.
#[derive(Parser, Debug)]
pub struct Command {
    /// Path to the secret key file for discovery.
    pub discovery_secret: PathBuf,

    /// Optional IP address to include in the enode URL.
    ///
    /// If not provided, defaults to 0.0.0.0.
    #[arg(long)]
    pub ip: Option<IpAddr>,
}

impl Command {
    /// Execute the enode command.
    pub fn execute(self) -> eyre::Result<()> {
        let sk = get_secret_key(&self.discovery_secret)?;
        let ip = self.ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let addr = SocketAddr::new(ip, 30303);
        let enr = NodeRecord::from_secret_key(addr, &sk);
        println!("{enr}");
        Ok(())
    }
}
