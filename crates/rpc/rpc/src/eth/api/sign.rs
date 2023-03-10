//! Contains RPC handler implementations specific to sign endpoints
use std::ops::Deref;

use crate::{
    eth::{
        error::{EthApiError, EthResult},
        signer::EthSigner,
    },
    EthApi,
};
use ethers_core::types::transaction::eip712::TypedData;
use reth_primitives::{Address, Bytes, Signature};
use serde_json::Value;

impl<Client, Pool, Network> EthApi<Client, Pool, Network> {
    pub(crate) async fn sign(&self, account: Address, message: Bytes) -> EthResult<Signature> {
        let signer = self.find_signer(&account).ok_or(EthApiError::UnknownAccount)?;
        let signature =
            signer.sign(account, &message).await.map_err(|_err| EthApiError::UnknownAccount)?; // Maybe this should be another error?
        Ok(signature)
    }

    pub(crate) async fn sign_typed_data(
        &self,
        data: Value,
        account: Address,
    ) -> EthResult<Signature> {
        let signer = self.find_signer(&account).ok_or(EthApiError::UnknownAccount)?;
        let data = serde_json::from_value::<TypedData>(data).unwrap();
        Ok(signer.sign_typed_data(account, &data).unwrap())
    }
    pub(crate) fn find_signer(&self, account: &Address) -> Option<&(dyn EthSigner + 'static)> {
        self.inner
            .signers
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| signer.deref())
    }
}
