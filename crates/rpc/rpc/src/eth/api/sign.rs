//! Contains RPC handler implementations specific to sign endpoints
use std::ops::Deref;

use crate::{
    eth::{
        error::{EthResult, SignError},
        signer::EthSigner
    },
    EthApi,
};
use ethers_core::types::transaction::eip712::TypedData;
use reth_primitives::{Address, Bytes, Signature};
use serde_json::Value;

impl<Client, Pool, Network> EthApi<Client, Pool, Network> {
    pub(crate) async fn sign(&self, account: Address, message: Bytes) -> EthResult<Signature> {
        let signer = self.find_signer(&account)?;
        let signature =
            signer.sign(account, &message).await?;
        Ok(signature)
    }

    pub(crate) async fn sign_typed_data(
        &self,
        data: Value,
        account: Address,
    ) -> EthResult<Signature> {
        let signer = self.find_signer(&account)?;
        let data = serde_json::from_value::<TypedData>(data).map_err(|_| SignError::TypedData)?;
        let signature = signer.sign_typed_data(account, &data)?;
        Ok(signature)
    }
    pub(crate) fn find_signer(&self, account: &Address) -> Result<&(dyn EthSigner + 'static), SignError>  {
        self.inner
            .signers
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| signer.deref())
            .ok_or(SignError::NoAccount)
    }
}
