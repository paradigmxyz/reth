//! Bootnodes for the network
//!
//! Ethereum bootnodes come from <https://github.com/ledgerwatch/erigon/blob/devel/params/bootnodes.go>

use crate::NodeRecord;
use alloc::vec::Vec;

/// Ethereum Foundation Go Bootnodes
pub static MAINNET_BOOTNODES: [&str; 5] = [
    "enode://ca967418ba165105303cfbb733dfb92bfcab80d65009d5e5f158c8e9e5f2c90795ae396a28d2114d66b4001e123e8c2c0465b018aed619fff41faca2ab4d2e64@212.99.218.66:0?discport=20151", // nodeops-bootnode-dcl1-01
    "enode://de4265bc38cba8508a14226356fabf480b88da880c19da49df57806a5b242f2ada08c25e54725c5ce1a762436c8f89e91781787c6b153ef9da72d00b62048170@129.212.166.61:0?discport=30303", // nodeops-bootnode-sfo3-01
    "enode://0b75ce940a50f9b3f37cfa162120642d16a4584774317fdaa02d0eec8f8aaf11fac79ba9d4213f33da9d4c512a3e0716a286b1b8f715c0a2cf5ac630030e0672@144.126.252.24:0?discport=30303", // nodeops-bootnode-blr1-01
    "enode://01472729e328876c6ccea993cf326b8a1ac969d4da4eac57ffa6b79d2d8338fa21012ff6c05b7ed78811a2f91768944219d9a3f0d33cb97ee0c128a8ed26386b@178.156.215.140:0?discport=30303", // nodeops-bootnode-ash-01
    "enode://0c949a7bc8d71b95ab9889c11e481a4743314093d5a0de5871de27bd8294e884b8e3b230b7cecfaf421fa6617aeff897e414444d7b4bf5b249dc9604716077ba@5.223.94.81:0?discport=30303", // nodeops-bootnode-sin-01
];

/// Ethereum Foundation Sepolia Bootnodes
pub static SEPOLIA_BOOTNODES: [&str; 5] = [
    "enode://4aff27bd8f1f667a56be304fcab797b4d7b630bf78581ffe6bc0d84852bb73362361ab2884ad396418abe25e7da621be0cadf5b02b6709b49d76b404813c9ddc@212.99.218.66:0?discport=20152", // nodeops-bootnode-dcl1-01
    "enode://665565ef7b9734bafb27fda8234ca43ca51ea097d0f29d9c9340daee0437c9e1d409c8283d1ce135eedccc2850f94c39c3cc63c641ad5fb0bef9dd37bd0fa6c1@129.212.166.61:0?discport=30403", // nodeops-bootnode-sfo3-01
    "enode://8e41eb6b03ef7b4c42d4cee19e8150f5fdc1ca28d9e33a627d09875e493a2bedfdea0bfe8c5bab778929a5334a311ca8d37e9016928fc6141c8fe52e708fbd98@144.126.252.24:0?discport=30403", // nodeops-bootnode-blr1-01
    "enode://b1e27df0cb42adc27b990879a5c4c99ce1bd15bdf0b829afdfdec50fbe352ad602075607f488ca805781e7f127d9709117a8bf8763ebac6e769f7cf50ded3806@178.156.215.140:0?discport=30403", // nodeops-bootnode-ash-01
    "enode://02dc5303f128bd0c8055a1fb126c9929bd74d401fca2f2aeba5d74208d1854b08cd5e7932d2db39e21891bf0347599b2cb63bedf14c96fac2bd92c77def82352@5.223.94.81:0?discport=30403", // nodeops-bootnode-sin-01
];

/// Ethereum Foundation Holesky Bootnodes
pub static HOLESKY_BOOTNODES: [&str; 2] = [
    "enode://ac906289e4b7f12df423d654c5a962b6ebe5b3a74cc9e06292a85221f9a64a6f1cfdd6b714ed6dacef51578f92b34c60ee91e9ede9c7f8fadc4d347326d95e2b@146.190.13.128:30303",
    "enode://a3435a0155a3e837c02f5e7f5662a2f1fbc25b48e4dc232016e1c51b544cb5b4510ef633ea3278c0e970fa8ad8141e2d4d0f9f95456c537ff05fdf9b31c15072@178.128.136.233:30303",
];

/// Ethereum Foundation Hoodi Bootnodes
/// From: <https://github.com/eth-clients/hoodi/blob/main/metadata/enodes.yaml>
pub static HOODI_BOOTNODES: [&str; 5] = [
    "enode://70bab91175f9bbcebbfbf155644f46c03ed44ef4927d816e3601ba5fd32f7a240ffb8953e737dd8595cf3773a62230ca27dd5390d268570f980f7d47a40dd1a3@212.99.218.66:0?discport=20153", // nodeops-bootnode-dcl1-01
    "enode://afd50407db4562c5049f3af489743a24b732cba6af2195e62205be59ae645aa0a4179da81515bfcfd20d3fd1f12ffdfcf5d4853ff1ad39189c3bb97132cd0986@129.212.166.61:0?discport=30503", // nodeops-bootnode-sfo3-01
    "enode://f787af01c154fb0fe82798d5c72ca81e3f2a789798a476a6c93da7acbb3f58002e7494c916680035a5beed158092c10bb08b10917bbc3ac41784e7b01017b4a4@144.126.252.24:0?discport=30503", // nodeops-bootnode-blr1-01
    "enode://71c400e99baa31e91d8ea7bda5850ef143b9da94b07413dbd4c1c84c4b5fc474806234c123bc5e1a97b7ae3162115cad9ccdf290319b0710f9d40d81b967153e@178.156.215.140:0?discport=30503", // nodeops-bootnode-ash-01
    "enode://81aabd7223f345e713b7f355cc76680ba8ec3a7b098f100655a467e4049113586fb5ce052a6ef90033ec62b91ed63457c87aa49dd0f1338af963b85522176db7@5.223.94.81:0?discport=30503", // nodeops-bootnode-sin-01
];

/// Returns parsed mainnet nodes
pub fn mainnet_nodes() -> Vec<NodeRecord> {
    parse_nodes(&MAINNET_BOOTNODES[..])
}

/// Returns parsed sepolia nodes
pub fn sepolia_nodes() -> Vec<NodeRecord> {
    parse_nodes(&SEPOLIA_BOOTNODES[..])
}

/// Returns parsed holesky nodes
pub fn holesky_nodes() -> Vec<NodeRecord> {
    parse_nodes(&HOLESKY_BOOTNODES[..])
}

/// Returns parsed hoodi nodes
pub fn hoodi_nodes() -> Vec<NodeRecord> {
    parse_nodes(&HOODI_BOOTNODES[..])
}

/// Parses all the nodes
pub fn parse_nodes(nodes: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<NodeRecord> {
    nodes.into_iter().map(|s| s.as_ref().parse().unwrap()).collect()
}
