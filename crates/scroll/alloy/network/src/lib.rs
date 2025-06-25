#![doc = include_str!("../README.md")]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_consensus::{TxEnvelope, TxType, TypedTransaction};
pub use alloy_network::*;
use alloy_primitives::{Address, Bytes, ChainId, TxKind, U256};
use alloy_provider::fillers::{
    ChainIdFiller, GasFiller, JoinFill, NonceFiller, RecommendedFillers,
};
use alloy_rpc_types_eth::AccessList;
use scroll_alloy_consensus::{self, ScrollTxEnvelope, ScrollTxType, ScrollTypedTransaction};
use scroll_alloy_rpc_types::ScrollTransactionRequest;

/// Types for a Scroll-stack network.
#[derive(Clone, Copy, Debug)]
pub struct Scroll {
    _private: (),
}

impl Network for Scroll {
    type TxType = ScrollTxType;

    type TxEnvelope = scroll_alloy_consensus::ScrollTxEnvelope;

    type UnsignedTx = scroll_alloy_consensus::ScrollTypedTransaction;

    type ReceiptEnvelope = scroll_alloy_consensus::ScrollReceiptEnvelope;

    type Header = alloy_consensus::Header;

    type TransactionRequest = scroll_alloy_rpc_types::ScrollTransactionRequest;

    type TransactionResponse = scroll_alloy_rpc_types::Transaction;

    type ReceiptResponse = scroll_alloy_rpc_types::ScrollTransactionReceipt;

    type HeaderResponse = alloy_rpc_types_eth::Header;

    type BlockResponse =
        alloy_rpc_types_eth::Block<Self::TransactionResponse, Self::HeaderResponse>;
}

impl TransactionBuilder<Scroll> for ScrollTransactionRequest {
    fn chain_id(&self) -> Option<ChainId> {
        self.as_ref().chain_id()
    }

    fn set_chain_id(&mut self, chain_id: ChainId) {
        self.as_mut().set_chain_id(chain_id);
    }

    fn nonce(&self) -> Option<u64> {
        self.as_ref().nonce()
    }

    fn set_nonce(&mut self, nonce: u64) {
        self.as_mut().set_nonce(nonce);
    }

    fn input(&self) -> Option<&Bytes> {
        self.as_ref().input()
    }

    fn set_input<T: Into<Bytes>>(&mut self, input: T) {
        self.as_mut().set_input(input);
    }

    fn from(&self) -> Option<Address> {
        self.as_ref().from()
    }

    fn set_from(&mut self, from: Address) {
        self.as_mut().set_from(from);
    }

    fn kind(&self) -> Option<TxKind> {
        self.as_ref().kind()
    }

    fn clear_kind(&mut self) {
        self.as_mut().clear_kind();
    }

    fn set_kind(&mut self, kind: TxKind) {
        self.as_mut().set_kind(kind);
    }

    fn value(&self) -> Option<U256> {
        self.as_ref().value()
    }

    fn set_value(&mut self, value: U256) {
        self.as_mut().set_value(value);
    }

    fn gas_price(&self) -> Option<u128> {
        self.as_ref().gas_price()
    }

    fn set_gas_price(&mut self, gas_price: u128) {
        self.as_mut().set_gas_price(gas_price);
    }

    fn max_fee_per_gas(&self) -> Option<u128> {
        self.as_ref().max_fee_per_gas()
    }

    fn set_max_fee_per_gas(&mut self, max_fee_per_gas: u128) {
        self.as_mut().set_max_fee_per_gas(max_fee_per_gas);
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.as_ref().max_priority_fee_per_gas()
    }

    fn set_max_priority_fee_per_gas(&mut self, max_priority_fee_per_gas: u128) {
        self.as_mut().set_max_priority_fee_per_gas(max_priority_fee_per_gas);
    }

    fn gas_limit(&self) -> Option<u64> {
        self.as_ref().gas_limit()
    }

    fn set_gas_limit(&mut self, gas_limit: u64) {
        self.as_mut().set_gas_limit(gas_limit);
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.as_ref().access_list()
    }

    fn set_access_list(&mut self, access_list: AccessList) {
        self.as_mut().set_access_list(access_list);
    }

    fn complete_type(&self, ty: ScrollTxType) -> Result<(), Vec<&'static str>> {
        match ty {
            ScrollTxType::L1Message => Err(vec!["not implemented for L1 message tx"]),
            _ => {
                let ty = TxType::try_from(ty as u8).unwrap();
                self.as_ref().complete_type(ty)
            }
        }
    }

    fn can_submit(&self) -> bool {
        self.as_ref().can_submit()
    }

    fn can_build(&self) -> bool {
        self.as_ref().can_build()
    }

    #[doc(alias = "output_transaction_type")]
    fn output_tx_type(&self) -> ScrollTxType {
        match self.as_ref().preferred_type() {
            TxType::Eip1559 | TxType::Eip4844 => ScrollTxType::Eip1559,
            TxType::Eip2930 => ScrollTxType::Eip2930,
            TxType::Legacy => ScrollTxType::Legacy,
            TxType::Eip7702 => ScrollTxType::Eip7702,
        }
    }

    #[doc(alias = "output_transaction_type_checked")]
    fn output_tx_type_checked(&self) -> Option<ScrollTxType> {
        self.as_ref().buildable_type().map(|tx_ty| match tx_ty {
            TxType::Eip1559 | TxType::Eip4844 => ScrollTxType::Eip1559,
            TxType::Eip2930 => ScrollTxType::Eip2930,
            TxType::Legacy => ScrollTxType::Legacy,
            TxType::Eip7702 => ScrollTxType::Eip7702,
        })
    }

    fn prep_for_submission(&mut self) {
        self.as_mut().prep_for_submission();
    }

    fn build_unsigned(self) -> BuildResult<ScrollTypedTransaction, Scroll> {
        if let Err((tx_type, missing)) = self.as_ref().missing_keys() {
            let tx_type = ScrollTxType::try_from(tx_type as u8).unwrap();
            return Err(TransactionBuilderError::InvalidTransactionRequest(tx_type, missing)
                .into_unbuilt(self));
        }
        Ok(self.build_typed_tx().expect("checked by missing_keys"))
    }

    async fn build<W: NetworkWallet<Scroll>>(
        self,
        wallet: &W,
    ) -> Result<<Scroll as Network>::TxEnvelope, TransactionBuilderError<Scroll>> {
        Ok(wallet.sign_request(self).await?)
    }
}

impl NetworkWallet<Scroll> for EthereumWallet {
    fn default_signer_address(&self) -> Address {
        NetworkWallet::<Ethereum>::default_signer_address(self)
    }

    fn has_signer_for(&self, address: &Address) -> bool {
        NetworkWallet::<Ethereum>::has_signer_for(self, address)
    }

    fn signer_addresses(&self) -> impl Iterator<Item = Address> {
        NetworkWallet::<Ethereum>::signer_addresses(self)
    }

    async fn sign_transaction_from(
        &self,
        sender: Address,
        tx: ScrollTypedTransaction,
    ) -> alloy_signer::Result<ScrollTxEnvelope> {
        let tx = match tx {
            ScrollTypedTransaction::Legacy(tx) => TypedTransaction::Legacy(tx),
            ScrollTypedTransaction::Eip2930(tx) => TypedTransaction::Eip2930(tx),
            ScrollTypedTransaction::Eip1559(tx) => TypedTransaction::Eip1559(tx),
            ScrollTypedTransaction::Eip7702(tx) => TypedTransaction::Eip7702(tx),
            ScrollTypedTransaction::L1Message(_) => {
                return Err(alloy_signer::Error::other("not implemented for deposit tx"))
            }
        };
        let tx = NetworkWallet::<Ethereum>::sign_transaction_from(self, sender, tx).await?;

        Ok(match tx {
            TxEnvelope::Eip1559(tx) => ScrollTxEnvelope::Eip1559(tx),
            TxEnvelope::Eip2930(tx) => ScrollTxEnvelope::Eip2930(tx),
            TxEnvelope::Eip7702(tx) => ScrollTxEnvelope::Eip7702(tx),
            TxEnvelope::Legacy(tx) => ScrollTxEnvelope::Legacy(tx),
            _ => unreachable!(),
        })
    }
}

impl RecommendedFillers for Scroll {
    type RecommendedFillers = JoinFill<GasFiller, JoinFill<NonceFiller, ChainIdFiller>>;

    fn recommended_fillers() -> Self::RecommendedFillers {
        Default::default()
    }
}
