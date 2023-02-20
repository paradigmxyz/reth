#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrestateAccount {
    pub balance: U256,
    pub nonce: U64,
    #[serde(serialize_with = "geth_alloc_compat")]
    pub storage: std::collections::BTreeMap<U256, U256>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Bytes>,
}

// todo: support rest of params
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrestateEnv {
    current_coinbase: Address,
    current_difficulty: U256,
    current_number: U64,
    current_timestamp: U256,
    current_gas_limit: U256,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
struct StateProviderTest {
    accounts: BTreeMap<Address, PrestateAccount>,
    contracts: HashMap<H256, Bytes>,
    block_hash: HashMap<U256, H256>,
}

// We implement a custom State Provider for the storage format provided by the alloc.json
//
impl AccountProvider for StateProviderTest {
    fn basic_account(&self, address: Address) -> reth_interfaces::Result<Option<Account>> {
        let ret = Ok(self.accounts.get(&address).map(|(_, acc)| *acc));
        ret
    }
}

impl BlockHashProvider for StateProviderTest {
    fn block_hash(&self, number: U256) -> reth_interfaces::Result<Option<H256>> {
        Ok(self.block_hash.get(&number).cloned())
    }
}

impl StateProvider for StateProviderTest {
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> reth_interfaces::Result<Option<reth_primitives::StorageValue>> {
        Ok(self.accounts.get(&account).and_then(|(storage, _)| storage.get(&storage_key).cloned()))
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> reth_interfaces::Result<Option<Bytes>> {
        Ok(self.contracts.get(&code_hash).cloned())
    }
}

use serde::Serializer;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx(Vec<Transaction>);

fn geth_alloc_compat<S>(
    value: &std::collections::BTreeMap<U256, U256>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.collect_map(
        value.iter().map(|(k, v)| (format!("0x{:0>64x}", k), format!("0x{:0>64x}", v))),
    )
}
