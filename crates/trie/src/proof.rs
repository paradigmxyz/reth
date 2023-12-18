use crate::{
    hashed_cursor::{HashedCursorFactory, HashedStorageCursor},
    node_iter::{AccountNode, AccountNodeIter, StorageNode, StorageNodeIter},
    prefix_set::PrefixSetMut,
    trie_cursor::{AccountTrieCursor, StorageTrieCursor},
    walker::TrieWalker,
    StateRootError, StorageRootError,
};
use alloy_rlp::{BufMut, Encodable};
use reth_db::{tables, transaction::DbTx};
use reth_primitives::{
    constants::EMPTY_ROOT_HASH,
    keccak256,
    trie::{AccountProof, HashBuilder, Nibbles, StorageProof, TrieAccount},
    Address, B256,
};

/// A struct for generating merkle proofs.
///
/// Proof generator adds the target address and slots to the prefix set, enables the proof retainer
/// on the hash builder and follows the same algorithm as the state root calculator.
/// See `StateRoot::root` for more info.
#[derive(Debug)]
pub struct Proof<'a, TX, H> {
    /// A reference to the database transaction.
    tx: &'a TX,
    /// The factory for hashed cursors.
    hashed_cursor_factory: H,
}

impl<'a, TX> Proof<'a, TX, &'a TX> {
    /// Create a new [Proof] instance.
    pub fn new(tx: &'a TX) -> Self {
        Self { tx, hashed_cursor_factory: tx }
    }
}

impl<'a, TX, H> Proof<'a, TX, H>
where
    TX: DbTx,
    H: HashedCursorFactory + Clone,
{
    /// Generate an account proof from intermediate nodes.
    pub fn account_proof(
        &self,
        address: Address,
        slots: &[B256],
    ) -> Result<AccountProof, StateRootError> {
        let target_hashed_address = keccak256(address);
        let target_nibbles = Nibbles::unpack(target_hashed_address);
        let mut account_proof = AccountProof::new(address);

        let hashed_account_cursor = self.hashed_cursor_factory.hashed_account_cursor()?;
        let trie_cursor = AccountTrieCursor::new(self.tx.cursor_read::<tables::AccountsTrie>()?);

        // Create the walker.
        let mut prefix_set = PrefixSetMut::default();
        prefix_set.insert(target_nibbles.clone());
        let walker = TrieWalker::new(trie_cursor, prefix_set.freeze());

        // Create a hash builder to rebuild the root node since it is not available in the database.
        let mut hash_builder =
            HashBuilder::default().with_proof_retainer(Vec::from([target_nibbles.clone()]));

        let mut account_rlp = Vec::with_capacity(128);
        let mut account_node_iter = AccountNodeIter::new(walker, hashed_account_cursor);
        while let Some(account_node) = account_node_iter.try_next()? {
            match account_node {
                AccountNode::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                AccountNode::Leaf(hashed_address, account) => {
                    let storage_root = if hashed_address == target_hashed_address {
                        let (storage_root, storage_proofs) =
                            self.storage_root_with_proofs(hashed_address, slots)?;
                        account_proof.set_account(account, storage_root, storage_proofs);
                        storage_root
                    } else {
                        self.storage_root(hashed_address)?
                    };

                    account_rlp.clear();
                    let account = TrieAccount::from((account, storage_root));
                    account.encode(&mut account_rlp as &mut dyn BufMut);

                    hash_builder.add_leaf(Nibbles::unpack(hashed_address), &account_rlp);
                }
            }
        }

        let _ = hash_builder.root();

        let proofs = hash_builder.take_proofs();
        account_proof.set_proof(proofs.values().cloned().collect());

        Ok(account_proof)
    }

    /// Compute storage root.
    pub fn storage_root(&self, hashed_address: B256) -> Result<B256, StorageRootError> {
        let (storage_root, _) = self.storage_root_with_proofs(hashed_address, &[])?;
        Ok(storage_root)
    }

    /// Compute the storage root and retain proofs for requested slots.
    pub fn storage_root_with_proofs(
        &self,
        hashed_address: B256,
        slots: &[B256],
    ) -> Result<(B256, Vec<StorageProof>), StorageRootError> {
        let mut hashed_storage_cursor = self.hashed_cursor_factory.hashed_storage_cursor()?;

        let mut proofs = slots.iter().copied().map(StorageProof::new).collect::<Vec<_>>();

        // short circuit on empty storage
        if hashed_storage_cursor.is_storage_empty(hashed_address)? {
            return Ok((EMPTY_ROOT_HASH, proofs))
        }

        let target_nibbles = proofs.iter().map(|p| p.nibbles.clone()).collect::<Vec<_>>();
        let prefix_set = PrefixSetMut::from(target_nibbles.clone()).freeze();
        let trie_cursor = StorageTrieCursor::new(
            self.tx.cursor_dup_read::<tables::StoragesTrie>()?,
            hashed_address,
        );
        let walker = TrieWalker::new(trie_cursor, prefix_set);

        let mut hash_builder = HashBuilder::default().with_proof_retainer(target_nibbles);
        let mut storage_node_iter =
            StorageNodeIter::new(walker, hashed_storage_cursor, hashed_address);
        while let Some(node) = storage_node_iter.try_next()? {
            match node {
                StorageNode::Branch(node) => {
                    hash_builder.add_branch(node.key, node.value, node.children_are_in_trie);
                }
                StorageNode::Leaf(hashed_slot, value) => {
                    let nibbles = Nibbles::unpack(hashed_slot);
                    if let Some(proof) = proofs.iter_mut().find(|proof| proof.nibbles == nibbles) {
                        proof.set_value(value);
                    }
                    hash_builder.add_leaf(nibbles, alloy_rlp::encode_fixed_size(&value).as_ref());
                }
            }
        }

        let root = hash_builder.root();

        let all_proof_nodes = hash_builder.take_proofs();
        for proof in proofs.iter_mut() {
            // Iterate over all proof nodes and find the matching ones.
            // The filtered results are guaranteed to be in order.
            let matching_proof_nodes = all_proof_nodes
                .iter()
                .filter(|(path, _)| proof.nibbles.starts_with(path))
                .map(|(_, node)| node.clone());
            proof.set_proof(matching_proof_nodes.collect());
        }

        Ok((root, proofs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateRoot;
    use once_cell::sync::Lazy;
    use reth_db::database::Database;
    use reth_interfaces::RethResult;
    use reth_primitives::{Account, Bytes, Chain, ChainSpec, StorageEntry, HOLESKY, MAINNET, U256};
    use reth_provider::{test_utils::create_test_provider_factory, HashingWriter, ProviderFactory};
    use std::{str::FromStr, sync::Arc};

    /*
        World State (sampled from <https://ethereum.stackexchange.com/questions/268/ethereum-block-architecture/6413#6413>)
        | address                                    | prefix    | hash                                                               | balance
        |--------------------------------------------|-----------|--------------------------------------------------------------------|--------
        | 0x2031f89b3ea8014eb51a78c316e42af3e0d7695f | 0xa711355 | 0xa711355ec1c8f7e26bb3ccbcb0b75d870d15846c0b98e5cc452db46c37faea40 |  45 eth
        | 0x33f0fc440b8477fcfbe9d0bf8649e7dea9baedb2 | 0xa77d337 | 0xa77d337781e762f3577784bab7491fcc43e291ce5a356b9bc517ac52eed3a37a |   1 wei
        | 0x62b0dd4aab2b1a0a04e279e2b828791a10755528 | 0xa7f9365 | 0xa7f936599f93b769acf90c7178fd2ddcac1b5b4bc9949ee5a04b7e0823c2446e | 1.1 eth
        | 0x1ed9b1dd266b607ee278726d324b855a093394a6 | 0xa77d397 | 0xa77d397a32b8ab5eb4b043c65b1f00c93f517bc8883c5cd31baf8e8a279475e3 | .12 eth

        All expected testspec results were obtained from querying proof RPC on the running geth instance `geth init crates/trie/testdata/proof-genesis.json && geth --http`.
    */
    static TEST_SPEC: Lazy<Arc<ChainSpec>> = Lazy::new(|| {
        ChainSpec {
            chain: Chain::Id(12345),
            genesis: serde_json::from_str(include_str!("../testdata/proof-genesis.json"))
                .expect("Can't deserialize test genesis json"),
            ..Default::default()
        }
        .into()
    });

    fn convert_to_proof<'a>(path: impl IntoIterator<Item = &'a str>) -> Vec<Bytes> {
        path.into_iter().map(Bytes::from_str).collect::<Result<Vec<_>, _>>().unwrap()
    }

    fn insert_genesis<DB: Database>(
        provider_factory: &ProviderFactory<DB>,
        chain_spec: Arc<ChainSpec>,
    ) -> RethResult<()> {
        let mut provider = provider_factory.provider_rw()?;

        // Hash accounts and insert them into hashing table.
        let genesis = chain_spec.genesis();
        let alloc_accounts =
            genesis.alloc.clone().into_iter().map(|(addr, account)| (addr, Some(account.into())));
        provider.insert_account_for_hashing(alloc_accounts).unwrap();

        let alloc_storage = genesis.alloc.clone().into_iter().filter_map(|(addr, account)| {
            // Only return `Some` if there is storage.
            account.storage.map(|storage| {
                (
                    addr,
                    storage
                        .into_iter()
                        .map(|(key, value)| StorageEntry { key, value: value.into() }),
                )
            })
        });
        provider.insert_storage_for_hashing(alloc_storage)?;

        let (_, updates) = StateRoot::new(provider.tx_ref())
            .root_with_updates()
            .map_err(Into::<reth_db::DatabaseError>::into)?;
        updates.flush(provider.tx_mut())?;

        provider.commit()?;

        Ok(())
    }

    #[test]
    fn testspec_proofs() {
        // Create test database and insert genesis accounts.
        let factory = create_test_provider_factory();
        insert_genesis(&factory, TEST_SPEC.clone()).unwrap();

        let data = Vec::from([
            (
                "0x2031f89b3ea8014eb51a78c316e42af3e0d7695f",
                convert_to_proof([
                    "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
                    "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
                    "0xf8719f31355ec1c8f7e26bb3ccbcb0b75d870d15846c0b98e5cc452db46c37faea40b84ff84d80890270801d946c940000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
                ])
            ),
            (
                "0x33f0fc440b8477fcfbe9d0bf8649e7dea9baedb2",
                convert_to_proof([
                    "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
                    "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
                    "0xe48200d3a0ef957210bca5b9b402d614eb8408c88cfbf4913eb6ab83ca233c8b8f0e626b54",
                    "0xf851808080a02743a5addaf4cf9b8c0c073e1eaa555deaaf8c41cb2b41958e88624fa45c2d908080808080a0bfbf6937911dfb88113fecdaa6bde822e4e99dae62489fcf61a91cb2f36793d680808080808080",
                    "0xf8679e207781e762f3577784bab7491fcc43e291ce5a356b9bc517ac52eed3a37ab846f8448001a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
                ])
            ),
            (
                "0x62b0dd4aab2b1a0a04e279e2b828791a10755528",
                convert_to_proof([
                    "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
                    "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
                    "0xf8709f3936599f93b769acf90c7178fd2ddcac1b5b4bc9949ee5a04b7e0823c2446eb84ef84c80880f43fc2c04ee0000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
                ])
            ),
            (
                "0x1ed9b1dd266b607ee278726d324b855a093394a6",
                convert_to_proof([
                    "0xe48200a7a040f916999be583c572cc4dd369ec53b0a99f7de95f13880cf203d98f935ed1b3",
                    "0xf87180a04fb9bab4bb88c062f32452b7c94c8f64d07b5851d44a39f1e32ba4b1829fdbfb8080808080a0b61eeb2eb82808b73c4ad14140a2836689f4ab8445d69dd40554eaf1fce34bc080808080808080a0dea230ff2026e65de419288183a340125b04b8405cc61627b3b4137e2260a1e880",
                    "0xe48200d3a0ef957210bca5b9b402d614eb8408c88cfbf4913eb6ab83ca233c8b8f0e626b54",
                    "0xf851808080a02743a5addaf4cf9b8c0c073e1eaa555deaaf8c41cb2b41958e88624fa45c2d908080808080a0bfbf6937911dfb88113fecdaa6bde822e4e99dae62489fcf61a91cb2f36793d680808080808080",
                    "0xf86f9e207a32b8ab5eb4b043c65b1f00c93f517bc8883c5cd31baf8e8a279475e3b84ef84c808801aa535d3d0c0000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
                ])
            ),
        ]);

        let provider = factory.provider().unwrap();
        for (target, expected_proof) in data {
            let target = Address::from_str(target).unwrap();
            let account_proof = Proof::new(provider.tx_ref()).account_proof(target, &[]).unwrap();
            pretty_assertions::assert_eq!(
                account_proof.proof,
                expected_proof,
                "proof for {target:?} does not match"
            );
        }
    }

    #[test]
    fn testspec_empty_storage_proof() {
        // Create test database and insert genesis accounts.
        let factory = create_test_provider_factory();
        insert_genesis(&factory, TEST_SPEC.clone()).unwrap();

        let target = Address::from_str("0x1ed9b1dd266b607ee278726d324b855a093394a6").unwrap();
        let slots = Vec::from([B256::with_last_byte(1), B256::with_last_byte(3)]);

        let provider = factory.provider().unwrap();
        let account_proof = Proof::new(provider.tx_ref()).account_proof(target, &slots).unwrap();
        assert_eq!(account_proof.storage_root, EMPTY_ROOT_HASH, "expected empty storage root");

        assert_eq!(slots.len(), account_proof.storage_proofs.len());
        for (idx, slot) in slots.into_iter().enumerate() {
            assert_eq!(account_proof.storage_proofs.get(idx), Some(&StorageProof::new(slot)));
        }
    }

    #[test]
    fn mainnet_genesis_account_proof() {
        // Create test database and insert genesis accounts.
        let factory = create_test_provider_factory();
        insert_genesis(&factory, MAINNET.clone()).unwrap();

        // Address from mainnet genesis allocation.
        // keccak256 - `0xcf67b71c90b0d523dd5004cf206f325748da347685071b34812e21801f5270c4`
        let target = Address::from_str("0x000d836201318ec6899a67540690382780743280").unwrap();

        // `cast proof 0x000d836201318ec6899a67540690382780743280 --block 0`
        let expected_account_proof = convert_to_proof([
            "0xf90211a090dcaf88c40c7bbc95a912cbdde67c175767b31173df9ee4b0d733bfdd511c43a0babe369f6b12092f49181ae04ca173fb68d1a5456f18d20fa32cba73954052bda0473ecf8a7e36a829e75039a3b055e51b8332cbf03324ab4af2066bbd6fbf0021a0bbda34753d7aa6c38e603f360244e8f59611921d9e1f128372fec0d586d4f9e0a04e44caecff45c9891f74f6a2156735886eedf6f1a733628ebc802ec79d844648a0a5f3f2f7542148c973977c8a1e154c4300fec92f755f7846f1b734d3ab1d90e7a0e823850f50bf72baae9d1733a36a444ab65d0a6faaba404f0583ce0ca4dad92da0f7a00cbe7d4b30b11faea3ae61b7f1f2b315b61d9f6bd68bfe587ad0eeceb721a07117ef9fc932f1a88e908eaead8565c19b5645dc9e5b1b6e841c5edbdfd71681a069eb2de283f32c11f859d7bcf93da23990d3e662935ed4d6b39ce3673ec84472a0203d26456312bbc4da5cd293b75b840fc5045e493d6f904d180823ec22bfed8ea09287b5c21f2254af4e64fca76acc5cd87399c7f1ede818db4326c98ce2dc2208a06fc2d754e304c48ce6a517753c62b1a9c1d5925b89707486d7fc08919e0a94eca07b1c54f15e299bd58bdfef9741538c7828b5d7d11a489f9c20d052b3471df475a051f9dd3739a927c89e357580a4c97b40234aa01ed3d5e0390dc982a7975880a0a089d613f26159af43616fd9455bb461f4869bfede26f2130835ed067a8b967bfb80",
            "0xf90211a0dae48f5b47930c28bb116fbd55e52cd47242c71bf55373b55eb2805ee2e4a929a00f1f37f337ec800e2e5974e2e7355f10f1a4832b39b846d916c3597a460e0676a0da8f627bb8fbeead17b318e0a8e4f528db310f591bb6ab2deda4a9f7ca902ab5a0971c662648d58295d0d0aa4b8055588da0037619951217c22052802549d94a2fa0ccc701efe4b3413fd6a61a6c9f40e955af774649a8d9fd212d046a5a39ddbb67a0d607cdb32e2bd635ee7f2f9e07bc94ddbd09b10ec0901b66628e15667aec570ba05b89203dc940e6fa70ec19ad4e01d01849d3a5baa0a8f9c0525256ed490b159fa0b84227d48df68aecc772939a59afa9e1a4ab578f7b698bdb1289e29b6044668ea0fd1c992070b94ace57e48cbf6511a16aa770c645f9f5efba87bbe59d0a042913a0e16a7ccea6748ae90de92f8aef3b3dc248a557b9ac4e296934313f24f7fced5fa042373cf4a00630d94de90d0a23b8f38ced6b0f7cb818b8925fee8f0c2a28a25aa05f89d2161c1741ff428864f7889866484cef622de5023a46e795dfdec336319fa07597a017664526c8c795ce1da27b8b72455c49657113e0455552dbc068c5ba31a0d5be9089012fda2c585a1b961e988ea5efcd3a06988e150a8682091f694b37c5a0f7b0352e38c315b2d9a14d51baea4ddee1770974c806e209355233c3c89dce6ea049bf6e8df0acafd0eff86defeeb305568e44d52d2235cf340ae15c6034e2b24180",
            "0xf901f1a0cf67e0f5d5f8d70e53a6278056a14ddca46846f5ef69c7bde6810d058d4a9eda80a06732ada65afd192197fe7ce57792a7f25d26978e64e954b7b84a1f7857ac279da05439f8d011683a6fc07efb90afca198fd7270c795c835c7c85d91402cda992eaa0449b93033b6152d289045fdb0bf3f44926f831566faa0e616b7be1abaad2cb2da031be6c3752bcd7afb99b1bb102baf200f8567c394d464315323a363697646616a0a40e3ed11d906749aa501279392ffde868bd35102db41364d9c601fd651f974aa0044bfa4fe8dd1a58e6c7144da79326e94d1331c0b00373f6ae7f3662f45534b7a098005e3e48db68cb1dc9b9f034ff74d2392028ddf718b0f2084133017da2c2e7a02a62bc40414ee95b02e202a9e89babbabd24bef0abc3fc6dcd3e9144ceb0b725a0239facd895bbf092830390a8676f34b35b29792ae561f196f86614e0448a5792a0a4080f88925daff6b4ce26d188428841bd65655d8e93509f2106020e76d41eefa04918987904be42a6894256ca60203283d1b89139cf21f09f5719c44b8cdbb8f7a06201fc3ef0827e594d953b5e3165520af4fceb719e11cc95fd8d3481519bfd8ca05d0e353d596bd725b09de49c01ede0f29023f0153d7b6d401556aeb525b2959ba0cd367d0679950e9c5f2aa4298fd4b081ade2ea429d71ff390c50f8520e16e30880",
            "0xf87180808080808080a0dbee8b33c73b86df839f309f7ac92eee19836e08b39302ffa33921b3c6a09f66a06068b283d51aeeee682b8fb5458354315d0b91737441ede5e137c18b4775174a8080808080a0fe7779c7d58c2fda43eba0a6644043c86ebb9ceb4836f89e30831f23eb059ece8080",
            "0xf8719f20b71c90b0d523dd5004cf206f325748da347685071b34812e21801f5270c4b84ff84d80890ad78ebc5ac6200000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        ]);

        let provider = factory.provider().unwrap();
        let account_proof = Proof::new(provider.tx_ref()).account_proof(target, &[]).unwrap();
        pretty_assertions::assert_eq!(account_proof.proof, expected_account_proof);
    }

    #[test]
    fn mainnet_genesis_account_proof_nonexistent() {
        // Create test database and insert genesis accounts.
        let factory = create_test_provider_factory();
        insert_genesis(&factory, MAINNET.clone()).unwrap();

        // Address that does not exist in mainnet genesis allocation.
        // keccak256 - `0x18f415ffd7f66bb1924d90f0e82fb79ca8c6d8a3473cd9a95446a443b9db1761`
        let target = Address::from_str("0x000d836201318ec6899a67540690382780743281").unwrap();

        // `cast proof 0x000d836201318ec6899a67540690382780743281 --block 0`
        let expected_account_proof = convert_to_proof([
            "0xf90211a090dcaf88c40c7bbc95a912cbdde67c175767b31173df9ee4b0d733bfdd511c43a0babe369f6b12092f49181ae04ca173fb68d1a5456f18d20fa32cba73954052bda0473ecf8a7e36a829e75039a3b055e51b8332cbf03324ab4af2066bbd6fbf0021a0bbda34753d7aa6c38e603f360244e8f59611921d9e1f128372fec0d586d4f9e0a04e44caecff45c9891f74f6a2156735886eedf6f1a733628ebc802ec79d844648a0a5f3f2f7542148c973977c8a1e154c4300fec92f755f7846f1b734d3ab1d90e7a0e823850f50bf72baae9d1733a36a444ab65d0a6faaba404f0583ce0ca4dad92da0f7a00cbe7d4b30b11faea3ae61b7f1f2b315b61d9f6bd68bfe587ad0eeceb721a07117ef9fc932f1a88e908eaead8565c19b5645dc9e5b1b6e841c5edbdfd71681a069eb2de283f32c11f859d7bcf93da23990d3e662935ed4d6b39ce3673ec84472a0203d26456312bbc4da5cd293b75b840fc5045e493d6f904d180823ec22bfed8ea09287b5c21f2254af4e64fca76acc5cd87399c7f1ede818db4326c98ce2dc2208a06fc2d754e304c48ce6a517753c62b1a9c1d5925b89707486d7fc08919e0a94eca07b1c54f15e299bd58bdfef9741538c7828b5d7d11a489f9c20d052b3471df475a051f9dd3739a927c89e357580a4c97b40234aa01ed3d5e0390dc982a7975880a0a089d613f26159af43616fd9455bb461f4869bfede26f2130835ed067a8b967bfb80",
            "0xf90211a0586b1ddec8db4824154209d355a1989b6c43aa69aba36e9d70c9faa53e7452baa0f86db47d628c73764d74b9ccaed73b8486d97a7731d57008fc9efaf417411860a0d9faed7b9ea107b5d98524246c977e782377f976e34f70717e8b1207f2f9b981a00218f59ccedf797c95e27c56405b9bf16845050fb43e773b66b26bc6992744f5a0dbf396f480c4e024156644adea7c331688d03742369e9d87ab8913bc439ff975a0aced524f39b22c62a5be512ddbca89f0b89b47c311065ccf423dee7013c7ea83a0c06b05f80b237b403adc019c0bc95b5de935021b14a75cbc18509eec60dfd83aa085339d45c4a52b7d523c301701f1ab339964e9c907440cff0a871c98dcf8811ea03ae9f6b8e227ec9be9461f0947b01696f78524c4519a6dee9fba14d209952cf9a0af17f551f9fa1ba4be41d0b342b160e2e8468d7e98a65a2dbf9d5fe5d6928024a0b850ac3bc03e9a309cc59ce5f1ab8db264870a7a22786081753d1db91897b8e6a09e796a4904bd78cb2655b5f346c94350e2d5f0dbf2bc00ac00871cd7ba46b241a0f6f0377427b900529caf32abf32ba1eb93f5f70153aa50b90bf55319a434c252a0725eaf27c8ee07e9b2511a6d6a0d71c649d855e8a9ed26e667903e2e94ae47cba0e4139fb48aa1a524d47f6e0df80314b88b52202d7e853da33c276aa8572283a8a05e9003d54a45935fdebae3513dc7cd16626dc05e1d903ae7f47f1a35aa6e234580",
            "0xf901d1a0b7c55b381eb205712a2f5d1b7d6309ac725da79ab159cb77dc2783af36e6596da0b3b48aa390e0f3718b486ccc32b01682f92819e652315c1629058cd4d9bb1545a0e3c0cc68af371009f14416c27e17f05f4f696566d2ba45362ce5711d4a01d0e4a0bad1e085e431b510508e2a9e3712633a414b3fe6fd358635ab206021254c1e10a0f8407fe8d5f557b9e012d52e688139bd932fec40d48630d7ff4204d27f8cc68da08c6ca46eff14ad4950e65469c394ca9d6b8690513b1c1a6f91523af00082474c80a0630c034178cb1290d4d906edf28688804d79d5e37a3122c909adab19ac7dc8c5a059f6d047c5d1cc75228c4517a537763cb410c38554f273e5448a53bc3c7166e7a0d842f53ce70c3aad1e616fa6485d3880d15c936fcc306ec14ae35236e5a60549a0218ee2ee673c69b4e1b953194b2568157a69085b86e4f01644fa06ab472c6cf9a016a35a660ea496df7c0da646378bfaa9562f401e42a5c2fe770b7bbe22433585a0dd0fbbe227a4d50868cdbb3107573910fd97131ea8d835bef81d91a2fc30b175a06aafa3d78cf179bf055bd5ec629be0ff8352ce0aec9125a4d75be3ee7eb71f10a01d6817ef9f64fcbb776ff6df0c83138dcd2001bd752727af3e60f4afc123d8d58080"
        ]);

        let provider = factory.provider().unwrap();
        let account_proof = Proof::new(provider.tx_ref()).account_proof(target, &[]).unwrap();
        pretty_assertions::assert_eq!(account_proof.proof, expected_account_proof);
    }

    #[test]
    fn holesky_deposit_contract_proof() {
        // Create test database and insert genesis accounts.
        let factory = create_test_provider_factory();
        insert_genesis(&factory, HOLESKY.clone()).unwrap();

        let target = Address::from_str("0x4242424242424242424242424242424242424242").unwrap();
        // existent
        let slot_22 =
            B256::from_str("0x0000000000000000000000000000000000000000000000000000000000000022")
                .unwrap();
        let slot_23 =
            B256::from_str("0x0000000000000000000000000000000000000000000000000000000000000023")
                .unwrap();
        let slot_24 =
            B256::from_str("0x0000000000000000000000000000000000000000000000000000000000000024")
                .unwrap();
        // non-existent
        let slot_100 =
            B256::from_str("0x0000000000000000000000000000000000000000000000000000000000000100")
                .unwrap();
        let slots = Vec::from([slot_22, slot_23, slot_24, slot_100]);

        // `cast proof 0x4242424242424242424242424242424242424242 0x22 0x23 0x24 0x100 --block 0`
        let expected = AccountProof {
            address: target,
            info:  Some(Account {
                balance: U256::ZERO,
                nonce: 0,
                bytecode_hash: Some(B256::from_str("0x2034f79e0e33b0ae6bef948532021baceb116adf2616478703bec6b17329f1cc").unwrap())
            }),
            storage_root: B256::from_str("0x556a482068355939c95a3412bdb21213a301483edb1b64402fb66ac9f3583599").unwrap(),
            proof: convert_to_proof([
                "0xf90211a0ea92fb71507739d5afe328d607b2c5e98322b7aa7cdfeccf817543058b54af70a0bd0c2525b5bee47abf7120c9e01ec3249699d687f80ebb96ed9ad9de913dbab0a0ab4b14b89416eb23c6b64204fa45cfcb39d4220016a9cd0815ebb751fe45eb71a0986ae29c2148b9e61f9a7543f44a1f8d029f1c5095b359652e9ec94e64b5d393a0555d54aa23ed990b0488153418637df7b2c878b604eb761aa2673b609937b0eba0140afb6a3909cc6047b3d44af13fc83f161a7e4c4ddba430a2841862912eb222a031b1185c1f455022d9e42ce04a71f174eb9441b1ada67449510500f4d85b3b22a051ecd01e18113b23cc65e62f67d69b33ee15d20bf81a6b524f7df90ded00ca15a0703769d6a7befad000bc2b4faae3e41b809b1b1241fe2964262554e7e3603488a0e5de7f600e4e6c3c3e5630e0c66f50506a17c9715642fccb63667e81397bbf93a095f783cd1d464a60e3c8adcadc28c6eb9fec7306664df39553be41dccc909606a04225fda3b89f0c59bf40129d1d5e5c3bf67a2129f0c55e53ffdd2cebf185d644a078e0f7fd3ae5a9bc90f66169614211b48fe235eb64818b3935d3e69c53523b9aa0a870e00e53ebaa1e9ec16e5f36606fd7d21d3a3c96894c0a2a23550949d4fdf7a0809226b69cee1f4f22ced1974e7805230da1909036a49a7652428999431afac2a0f11593b2407e86e11997325d8df2d22d937bbe0aef8302ba40c6be0601b04fc380",
                "0xf901f1a09da7d9755fe0c558b3c3de9fdcdf9f28ae641f38c9787b05b73ab22ae53af3e2a0d9990bf0b810d1145ecb2b011fd68c63cc85564e6724166fd4a9520180706e5fa05f5f09855df46330aa310e8d6be5fb82d1a4b975782d9b29acf06ac8d3e72b1ca0ca976997ddaf06f18992f6207e4f6a05979d07acead96568058789017cc6d06ba04d78166b48044fdc28ed22d2fd39c8df6f8aaa04cb71d3a17286856f6893ff83a004f8c7cc4f1335182a1709fb28fc67d52e59878480210abcba864d5d1fd4a066a0fc3b71c33e2e6b77c5e494c1db7fdbb447473f003daf378c7a63ba9bf3f0049d80a07b8e7a21c1178d28074f157b50fca85ee25c12568ff8e9706dcbcdacb77bf854a0973274526811393ea0bf4811ca9077531db00d06b86237a2ecd683f55ba4bcb0a03a93d726d7487874e51b52d8d534c63aa2a689df18e3b307c0d6cb0a388b00f3a06aa67101d011d1c22fe739ef83b04b5214a3e2f8e1a2625d8bfdb116b447e86fa02dd545b33c62d33a183e127a08a4767fba891d9f3b94fc20a2ca02600d6d1fffa0f3b039a4f32349e85c782d1164c1890e5bf16badc9ee4cf827db6afd2229dde6a0d9240a9d2d5851d05a97ff3305334dfdb0101e1e321fc279d2bb3cad6afa8fc8a01b69c6ab5173de8a8ec53a6ebba965713a4cc7feb86cb3e230def37c230ca2b280",
                "0xf869a0202a47fc6863b89a6b51890ef3c1550d560886c027141d2058ba1e2d4c66d99ab846f8448080a0556a482068355939c95a3412bdb21213a301483edb1b64402fb66ac9f3583599a02034f79e0e33b0ae6bef948532021baceb116adf2616478703bec6b17329f1cc"
            ]),
            storage_proofs: Vec::from([
                StorageProof {
                    key: slot_22,
                    nibbles: Nibbles::unpack(keccak256(slot_22)),
                    value: U256::from_str("0xf5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b").unwrap(),
                    proof: convert_to_proof([
                        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
                        "0xf85180a0776aa456ba9c5008e03b82b841a9cf2fc1e8578cfacd5c9015804eae315f17fb80808080808080808080808080a072e3e284d47badbb0a5ca1421e1179d3ea90cc10785b26b74fb8a81f0f9e841880",
                        "0xf843a020035b26e3e9eee00e0d72fd1ee8ddca6894550dca6916ea2ac6baa90d11e510a1a0f5a5fd42d16a20302798ef6ed309979b43003d2320d9f0e8ea9831a92759fb4b"
                    ])
                },
                StorageProof {
                    key: slot_23,
                    nibbles: Nibbles::unpack(keccak256(slot_23)),
                    value: U256::from_str("0xdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71").unwrap(),
                    proof: convert_to_proof([
                        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
                        "0xf8518080808080a0d546c4ca227a267d29796643032422374624ed109b3d94848c5dc06baceaee76808080808080a027c48e210ccc6e01686be2d4a199d35f0e1e8df624a8d3a17c163be8861acd6680808080",
                        "0xf843a0207b2b5166478fd4318d2acc6cc2c704584312bdd8781b32d5d06abda57f4230a1a0db56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71"
                    ])
                },
                StorageProof {
                    key: slot_24,
                    nibbles: Nibbles::unpack(keccak256(slot_24)),
                    value: U256::from_str("0xc78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c").unwrap(),
                    proof: convert_to_proof([
                        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
                        "0xf85180808080a030263404acfee103d0b1019053ff3240fce433c69b709831673285fa5887ce4c80808080808080a0f8f1fbb1f7b482d9860480feebb83ff54a8b6ec1ead61cc7d2f25d7c01659f9c80808080",
                        "0xf843a020d332d19b93bcabe3cce7ca0c18a052f57e5fd03b4758a09f30f5ddc4b22ec4a1a0c78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c"
                    ])
                },
                StorageProof {
                    key: slot_100,
                    nibbles: Nibbles::unpack(keccak256(slot_100)),
                    value: U256::ZERO,
                    proof: convert_to_proof([
                        "0xf9019180a0aafd5b14a6edacd149e110ba6776a654f2dbffca340902be933d011113f2750380a0a502c93b1918c4c6534d4593ae03a5a23fa10ebc30ffb7080b297bff2446e42da02eb2bf45fd443bd1df8b6f9c09726a4c6252a0f7896a131a081e39a7f644b38980a0a9cf7f673a0bce76fd40332afe8601542910b48dea44e93933a3e5e930da5d19a0ddf79db0a36d0c8134ba143bcb541cd4795a9a2bae8aca0ba24b8d8963c2a77da0b973ec0f48f710bf79f63688485755cbe87f9d4c68326bb83c26af620802a80ea0f0855349af6bf84afc8bca2eda31c8ef8c5139be1929eeb3da4ba6b68a818cb0a0c271e189aeeb1db5d59d7fe87d7d6327bbe7cfa389619016459196497de3ccdea0e7503ba5799e77aa31bbe1310c312ca17b2c5bcc8fa38f266675e8f154c2516ba09278b846696d37213ab9d20a5eb42b03db3173ce490a2ef3b2f3b3600579fc63a0e9041059114f9c910adeca12dbba1fef79b2e2c8899f2d7213cd22dfe4310561a047c59da56bb2bf348c9dd2a2e8f5538a92b904b661cfe54a4298b85868bbe4858080",
                        "0xf891a090bacef44b189ddffdc5f22edc70fe298c58e5e523e6e1dfdf7dbc6d657f7d1b80a026eed68746028bc369eb456b7d3ee475aa16f34e5eaa0c98fdedb9c59ebc53b0808080a09ce86197173e14e0633db84ce8eea32c5454eebe954779255644b45b717e8841808080a0328c7afb2c58ef3f8c4117a8ebd336f1a61d24591067ed9c5aae94796cac987d808080808080"
                    ])
                },
            ])
        };

        let provider = factory.provider().unwrap();
        let account_proof = Proof::new(provider.tx_ref()).account_proof(target, &slots).unwrap();
        pretty_assertions::assert_eq!(account_proof, expected);
    }
}
