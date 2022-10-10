//! Peer persistence utilities

use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use enr::{secp256k1::SecretKey, Enr};
use serde_derive::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// Peers that have been obtained discovery sources when the node is running
    pub discovered_peers: HashSet<Enr<SecretKey>>,

    /// Pre-determined peers to reach out to
    pub static_peers: HashSet<Enr<SecretKey>>,
}

impl Anchor {
    /// Adds peers to the discovered peer list
    pub fn add_discovered(&mut self, new_peers: Vec<Enr<SecretKey>>) {
        self.discovered_peers.extend(new_peers);
    }

    /// Adds peers to the static peer list.
    pub fn add_static(&mut self, new_peers: Vec<Enr<SecretKey>>) {
        self.static_peers.extend(new_peers);
    }

    /// Returns true if both peer lists are empty
    pub fn is_empty(&self) -> bool {
        self.static_peers.is_empty() && self.discovered_peers.is_empty()
    }
}

/// An error that can occur while parsing a peer list from a file
#[derive(Debug, Error)]
pub enum AnchorError {
    /// Error opening the anchor file
    #[error("Could not open or write to the anchor file: {0}")]
    IoError(#[from] std::io::Error),

    /// Error occurred when loading the anchor file from TOML
    #[error("Could not deserialize the peer list from TOML: {0}")]
    LoadError(#[from] toml::de::Error),

    /// Error occurred when saving the anchor file to TOML
    #[error("Could not serialize the peer list as TOML: {0}")]
    SaveError(#[from] toml::ser::Error),
}

/// A version of [`Anchor`] that is loaded from a TOML file and saves its contents when it is
/// dropped.
#[derive(Debug)]
pub struct PersistentAnchor {
    /// The list of addresses to persist
    anchor: Anchor,

    /// The Path to store the anchor file
    path: PathBuf,
}

impl PersistentAnchor {
    /// This will attempt to load the [`Anchor`] from a file, and if the file doesn't exist it will
    /// attempt to initialize it with an empty peer list.
    pub fn new_from_file(path: &Path) -> Result<Self, AnchorError> {
        let mut binding = OpenOptions::new();
        let rw_opts = binding.read(true).write(true);

        let mut file = if path.try_exists()? {
            rw_opts.open(path)?
        } else {
            rw_opts.create(true).open(path)?
        };

        let mut contents = String::new();
        file.read_to_string(&mut contents)?;

        // if the file exists but is empty then we should initialize the file format and return
        // an empty [`Anchor`]
        if contents.is_empty() {
            let mut anchor = Self { anchor: Anchor::default(), path: path.to_path_buf() };
            anchor.save_toml()?;
            return Ok(anchor)
        }

        let anchor: Anchor = toml::from_str(&contents)?;
        Ok(Self { anchor, path: path.to_path_buf() })
    }

    /// Save the contents of the [`Anchor`] into the associated file as TOML.
    pub fn save_toml(&mut self) -> Result<(), AnchorError> {
        let mut file = OpenOptions::new().read(true).write(true).create(true).open(&self.path)?;

        if !self.anchor.is_empty() {
            let anchor_contents = toml::to_string_pretty(&self.anchor)?;
            file.write_all(anchor_contents.as_bytes())?;
        }
        Ok(())
    }
}

impl Drop for PersistentAnchor {
    fn drop(&mut self) {
        if let Err(save_error) = self.save_toml() {
            error!("Could not save anchor to file: {}", save_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use enr::{
        secp256k1::{rand::thread_rng, SecretKey},
        EnrBuilder,
    };
    use std::{fs::remove_file, net::Ipv4Addr};
    use tempfile::tempdir;

    use super::{Anchor, PersistentAnchor};

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

    #[test]
    fn create_empty_anchor() {
        let file_name = "temp_anchor.toml";
        let temp_file_path = tempdir().unwrap().path().with_file_name(file_name);

        // this test's purpose is to make sure new_from_file works if the file doesn't exist
        assert!(!temp_file_path.exists());
        let persistent_anchor = PersistentAnchor::new_from_file(&temp_file_path);

        // make sure to clean up
        let anchor = persistent_anchor.unwrap();

        // need to drop the PersistentAnchor explicitly before cleanup or it will be saved
        drop(anchor);
        remove_file(&temp_file_path).unwrap();
    }

    #[test]
    fn save_temp_anchor() {
        let file_name = "temp_anchor_two.toml";
        let temp_file_path = tempdir().unwrap().path().with_file_name(file_name);

        let mut persistent_anchor = PersistentAnchor::new_from_file(&temp_file_path).unwrap();

        // add some ENRs to both lists
        let mut rng = thread_rng();

        let key = SecretKey::new(&mut rng);
        let ip = Ipv4Addr::new(192, 168, 1, 1);
        let enr = EnrBuilder::new("v4").ip4(ip).tcp4(8000).build(&key).unwrap();
        persistent_anchor.anchor.add_discovered(vec![enr]);

        let key = SecretKey::new(&mut rng);
        let ip = Ipv4Addr::new(192, 168, 1, 2);
        let enr = EnrBuilder::new("v4").ip4(ip).tcp4(8000).build(&key).unwrap();
        persistent_anchor.anchor.add_static(vec![enr]);

        // save the old struct before dropping
        let prev_anchor = persistent_anchor.anchor.clone();
        drop(persistent_anchor);

        // finally check file contents
        let new_persistent = PersistentAnchor::new_from_file(&temp_file_path).unwrap();
        let new_anchor = new_persistent.anchor.clone();

        // need to drop the PersistentAnchor explicitly before cleanup or it will be saved
        drop(new_persistent);
        remove_file(&temp_file_path).unwrap();
        assert_eq!(new_anchor, prev_anchor);
    }
}
