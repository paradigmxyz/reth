//! Enode identifier command

use clap::Parser;
use eyre::WrapErr;
use reth_network_peers::NodeRecord;
use secp256k1::SecretKey;
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

    /// Port to include in the enode URL.
    #[arg(long, default_value_t = 30303)]
    pub port: u16,
}

impl Command {
    /// Execute the enode command.
    pub fn execute(self) -> eyre::Result<()> {
        let contents = reth_fs_util::read_to_string(&self.discovery_secret)?;
        // Parse exactly like the node's key loader so only keys it accepts produce an enode.
        let sk = contents.parse::<SecretKey>().wrap_err_with(|| {
            format!("Invalid secret key in {}", self.discovery_secret.display())
        })?;
        let ip = self.ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let addr = SocketAddr::new(ip, self.port);
        let enr = NodeRecord::from_secret_key(addr, &sk);
        println!("{enr}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_secret_key_is_not_generated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        let cmd = Command::parse_from(["enode", path.to_str().unwrap()]);
        assert!(cmd.execute().is_err());
        assert!(!path.exists());
    }
}
