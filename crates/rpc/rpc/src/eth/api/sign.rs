//! Contains RPC handler implementations specific to sign endpoints
use std::ops::Deref;

use crate::{
    eth::{
        error::{EthApiError, EthResult},
        signer::EthSigner,
    },
    EthApi,
};
use reth_primitives::{Address, Bytes, Signature};
impl<Client, Pool, Network> EthApi<Client, Pool, Network> {
    pub(crate) async fn sign(&self, account: Address, message: Bytes) -> EthResult<Signature> {
        let signer = self.find_signer(&account).ok_or(EthApiError::UnknownAccount)?;
        let signature =
            signer.sign(account, &message).await.map_err(|_err| EthApiError::UnknownAccount)?;
        Ok(signature)
    }

    pub(crate) fn find_signer(&self, account: &Address) -> Option<&(dyn EthSigner + 'static)> {
        self.inner
            .signers
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| signer.deref())
    }
}
