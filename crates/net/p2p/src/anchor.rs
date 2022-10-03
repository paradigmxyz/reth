//! Peer persistence utilities

use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Read, Write},
    path::Path,
};

use enr::{secp256k1::SecretKey, Enr};
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;

// TODO: impl Hash for Enr<K> upstream and change to HashSet to prevent duplicates
// TODO: enforce one-to-one mapping between IP and key

/// Contains a list of peers to persist across node restarts.
///
/// Addresses in this list can come from:
///  * Discovery methods (discv4, discv5, dnsdisc)
///  * Known static peers
///
/// Updates to this list can come from:
///  * Discovery methods (discv4, discv5, dnsdisc)
///    * All peers we connect to from discovery methods are outbound peers.
///  * Inbound connections
///    * Updates for inbound connections must be based on the address advertised in the node record
///      for that peer, if a node record exists.
///    * Updates for inbound connections must NOT be based on the peer's remote address, since its
///      port may be ephemeral.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Anchor {
    /// Peers that have been obtained discovery sources when the node is running
    pub discovered_peers: Vec<Enr<SecretKey>>,

    /// Pre-determined peers to reach out to
    pub static_peers: Vec<Enr<SecretKey>>,
}

impl Anchor {
    /// Creates an empty [`Anchor`]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds peers to the discovered peer list
    pub fn add_discovered(&mut self, new_peers: Vec<Enr<SecretKey>>) {
        self.discovered_peers.extend(new_peers);
        self.discovered_peers.dedup();
    }

    /// Adds peers to the static peer list.
    pub fn add_static(&mut self, new_peers: Vec<Enr<SecretKey>>) {
        self.static_peers.extend(new_peers);
        self.static_peers.dedup();
    }
}

#[derive(Debug, Error)]
/// An error that can occur while parsing a peer list from a file
pub enum AnchorError {
    /// Error opening the anchor file
    #[error("error opening or writing the anchor file")]
    IoError(#[from] std::io::Error),

    /// Error occurred when loading the anchor file from TOML
    #[error("error deserializing the peer list from TOML")]
    LoadError(#[from] toml::de::Error),

    /// Error occurred when saving the anchor file to TOML
    #[error("error serializing the peer list as TOML")]
    SaveError(#[from] toml::ser::Error),
}

/// A version of [`Anchor`] that is loaded from a TOML file and saves its contents when it is
/// dropped.
#[derive(Debug)]
pub struct PersistentAnchor {
    /// The list of addresses to persist
    anchor: Anchor,

    /// The File handle for the anchor
    file: File,
}

impl PersistentAnchor {
    /// This will attempt to load the [`Anchor`] from a file, and if the file doesn't exist it will
    /// attempt to initialize it with an empty peer list.
    pub fn new_from_file<P: AsRef<Path>>(path: P) -> Result<Self, AnchorError> {
        let file = OpenOptions::new().write(true).create(true).open(path)?;
        Self::from_toml(file)
    }

    /// Load the [`Anchor`] from the given TOML file.
    pub fn from_toml(file: File) -> Result<Self, AnchorError> {
        let mut reader = BufReader::new(&file);
        let mut contents = String::new();
        reader.read_to_string(&mut contents)?;

        let anchor: Anchor = toml::from_str(&contents)?;
        Ok(Self { anchor, file })
    }

    /// Save the contents of the [`Anchor`] into the associated file as TOML.
    pub fn save_toml(&mut self) -> Result<(), AnchorError> {
        let anchor_contents = toml::to_vec(&self.anchor)?;
        self.file.write_all(anchor_contents.as_ref())?;
        Ok(())
    }
}

impl Drop for PersistentAnchor {
    fn drop(&mut self) {
        self.save_toml().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::Anchor;

    #[test]
    fn serde_read_toml() {
        let _: Anchor = toml::from_str(r#"
            discovered_peers = [
                "enr:-Iu4QGuiaVXBEoi4kcLbsoPYX7GTK9ExOODTuqYBp9CyHN_PSDtnLMCIL91ydxUDRPZ-jem-o0WotK6JoZjPQWhTfEsTgmlkgnY0gmlwhDbOLfeJc2VjcDI1NmsxoQLVqNEoCVTC74VmUx25USyFe7lL0TgpXHaCX9CDy9H6boN0Y3CCIyiDdWRwgiMo",
                "enr:-Iu4QLNTiVhgyDyvCBnewNcn9Wb7fjPoKYD2NPe-jDZ3_TqaGFK8CcWr7ai7w9X8Im_ZjQYyeoBP_luLLBB4wy39gQ4JgmlkgnY0gmlwhCOhiGqJc2VjcDI1NmsxoQMrmBYg_yR_ZKZKoLiChvlpNqdwXwodXmgw_TRow7RVwYN0Y3CCIyiDdWRwgiMo",
            ]
            static_peers = [
                "enr:-Iu4QLpJhdfRFsuMrAsFQOSZTIW1PAf7Ndg0GB0tMByt2-n1bwVgLsnHOuujMg-YLns9g1Rw8rfcw1KCZjQrnUcUdekNgmlkgnY0gmlwhA01ZgSJc2VjcDI1NmsxoQPk2OMW7stSjbdcMgrKEdFOLsRkIuxgBFryA3tIJM0YxYN0Y3CCIyiDdWRwgiMo",
                "enr:-Iu4QBHuAmMN5ogZP_Mwh_bADnIOS2xqj8yyJI3EbxW66WKtO_JorshNQJ1NY8zo-u3G7HQvGW3zkV6_kRx5d0R19bETgmlkgnY0gmlwhDRCMUyJc2VjcDI1NmsxoQJZ8jY1HYauxirnJkVI32FoN7_7KrE05asCkZb7nj_b-YN0Y3CCIyiDdWRwgiMo",
            ]
        "#).expect("Parsing valid TOML into an Anchor should not fail");
    }
}
