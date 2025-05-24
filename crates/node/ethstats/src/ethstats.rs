//! Support ethstats, the network stats reporting service

use crate::{connection::WsConnection, error::Error};

/// The ethstats service
#[derive(Debug, Clone)]
pub struct Service {
    /// TODO: Add providers to get the node and block info
    
    /// Name of the node to display on the monitoring page
    pub node: String,

    /// Password to authorize access to the monitoring page
    pub pass: String,

    /// Remote address of the monitoring service
    pub host: String,

    /// Port number of the monitoring service
    pub port: u16,
}

impl Service {
    /// Create a new ethstats service
    pub fn new(url: &str) -> Result<Self, Error> {
        // URL argument should be of the form <nodename:secret@host:port>
        // Split into credentials and address
        let (cred_part, addr_part) = url.split_once('@').ok_or(Error::InvalidUrl)?;

        // Split credentials: nodename:secret
        let (nodename, secret) = cred_part.split_once(':').ok_or(Error::InvalidUrl)?;

        // Split address: host:port
        let (host, port_str) = addr_part.split_once(':').ok_or(Error::InvalidUrl)?;
        let port: u16 = port_str.parse().map_err(|_| Error::InvalidPort)?;

        Ok(Self { node: nodename.to_string(), pass: secret.to_string(), host: host.to_string(), port })
    }

    pub fn loop_service(&self) {
        // TODO: Implement the loop
    }

    pub fn login(&self, socket: &mut WsConnection) {
        // TODO: Implement the login
    }

    fn report(&self, socket: &mut WsConnection) {
        // TODO: Implement the report
    }
}


