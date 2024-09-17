use verkle_trie::VerkleConfig;
use reth_verkle_db::VerkleDb;
use reth_provider::test_utils::create_test_provider_factory;


#[test]
fn test_trie_root_commitment() {
    use verkle_trie::{Trie, TrieTrait};
    let factory = create_test_provider_factory();
    let tx = factory.provider_rw().unwrap();
    let config = VerkleConfig::new(VerkleDb::new(tx.tx_ref()));
    let mut trie = Trie::new(config);

    let keys = vec![
        [
            245, 110, 100, 66, 36, 244, 87, 100, 144, 207, 224, 222, 20, 36, 164, 83, 34, 18, 82,
            155, 254, 55, 71, 19, 216, 78, 125, 126, 142, 146, 114, 0,
        ],
        [
            245, 110, 100, 66, 36, 244, 87, 100, 144, 207, 224, 222, 20, 36, 164, 83, 34, 18, 82,
            155, 254, 55, 71, 19, 216, 78, 125, 126, 142, 146, 114, 1,
        ],
        [
            245, 110, 100, 66, 36, 244, 87, 100, 144, 207, 224, 222, 20, 36, 164, 83, 34, 18, 82,
            155, 254, 55, 71, 19, 216, 78, 125, 126, 142, 146, 114, 2,
        ],
        [
            245, 110, 100, 66, 36, 244, 87, 100, 144, 207, 224, 222, 20, 36, 164, 83, 34, 18, 82,
            155, 254, 55, 71, 19, 216, 78, 125, 126, 142, 146, 114, 3,
        ],
        [
            245, 110, 100, 66, 36, 244, 87, 100, 144, 207, 224, 222, 20, 36, 164, 83, 34, 18, 82,
            155, 254, 55, 71, 19, 216, 78, 125, 126, 142, 146, 114, 4,
        ],
    ];

    let values = vec![
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
        [
            0, 0, 100, 167, 179, 182, 224, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0,
        ],
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
        [
            197, 210, 70, 1, 134, 247, 35, 60, 146, 126, 125, 178, 220, 199, 3, 192, 229, 0, 182,
            83, 202, 130, 39, 59, 123, 250, 216, 4, 93, 133, 164, 112,
        ],
        [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
    ];
    let key_vals = keys.into_iter().zip(values);

    trie.insert(key_vals);

    let root = trie.root_commitment();

    let expected = "10ed89d89047bb168baa4e69b8607e260049e928ddbcb2fdd23ea0f4182b1f8a";

    use banderwagon::trait_defs::*;
    let mut root_bytes = [0u8; 32];
    root.serialize_compressed(&mut root_bytes[..]).unwrap();
    assert_eq!(hex::encode(root_bytes), expected);
}