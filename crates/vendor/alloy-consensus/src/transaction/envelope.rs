use super::SignableTransaction;
use crate::{
    error::ValueError,
    transaction::{
        eip4844::{TxEip4844, TxEip4844Variant},
        RlpEcdsaEncodableTx, TxHashRef,
    },
    Signed, TransactionEnvelope, TxEip1559, TxEip2930, TxEip4844WithSidecar, TxEip7702, TxLegacy,
};
use alloy_eips::{eip2718::Encodable2718, eip7594::Encodable7594};
use alloy_primitives::{Bytes, Signature, B256};

/// The Ethereum [EIP-2718] Transaction Envelope.
///
/// # Note:
///
/// This enum distinguishes between tagged and untagged legacy transactions, as
/// the in-protocol merkle tree may commit to EITHER 0-prefixed or raw.
/// Therefore we must ensure that encoding returns the precise byte-array that
/// was decoded, preserving the presence or absence of the `TransactionType`
/// flag.
///
/// [EIP-2718]: https://eips.ethereum.org/EIPS/eip-2718
pub type TxEnvelope = EthereumTxEnvelope<TxEip4844Variant>;

impl<T: Encodable7594> EthereumTxEnvelope<TxEip4844Variant<T>> {
    /// Attempts to convert the envelope into the pooled variant.
    ///
    /// Returns an error if the envelope's variant is incompatible with the pooled format:
    /// [`crate::TxEip4844`] without the sidecar.
    pub fn try_into_pooled(
        self,
    ) -> Result<EthereumTxEnvelope<TxEip4844WithSidecar<T>>, ValueError<Self>> {
        match self {
            Self::Legacy(tx) => Ok(tx.into()),
            Self::Eip2930(tx) => Ok(tx.into()),
            Self::Eip1559(tx) => Ok(tx.into()),
            Self::Eip4844(tx) => EthereumTxEnvelope::try_from(tx).map_err(ValueError::convert),
            Self::Eip7702(tx) => Ok(tx.into()),
        }
    }
}

impl EthereumTxEnvelope<TxEip4844> {
    /// Attempts to convert the envelope into the pooled variant.
    ///
    /// Returns an error if the envelope's variant is incompatible with the pooled format:
    /// [`crate::TxEip4844`] without the sidecar.
    pub fn try_into_pooled<T>(
        self,
    ) -> Result<EthereumTxEnvelope<TxEip4844WithSidecar<T>>, ValueError<Self>> {
        match self {
            Self::Legacy(tx) => Ok(tx.into()),
            Self::Eip2930(tx) => Ok(tx.into()),
            Self::Eip1559(tx) => Ok(tx.into()),
            Self::Eip4844(tx) => {
                Err(ValueError::new(tx.into(), "pooled transaction requires 4844 sidecar"))
            }
            Self::Eip7702(tx) => Ok(tx.into()),
        }
    }

    /// Converts from an EIP-4844 transaction to a [`EthereumTxEnvelope<TxEip4844WithSidecar<T>>`]
    /// with the given sidecar.
    ///
    /// Returns an `Err` containing the original [`EthereumTxEnvelope`] if the transaction is not an
    /// EIP-4844 variant.
    pub fn try_into_pooled_eip4844<T>(
        self,
        sidecar: T,
    ) -> Result<EthereumTxEnvelope<TxEip4844WithSidecar<T>>, ValueError<Self>> {
        match self {
            Self::Eip4844(tx) => {
                Ok(EthereumTxEnvelope::Eip4844(tx.map(|tx| tx.with_sidecar(sidecar))))
            }
            this => Err(ValueError::new_static(this, "Expected 4844 transaction")),
        }
    }
}

impl<T> EthereumTxEnvelope<T> {
    /// Creates a new signed transaction from the given transaction, signature and hash.
    ///
    /// Caution: This assumes the given hash is the correct transaction hash.
    pub fn new_unchecked(
        transaction: EthereumTypedTransaction<T>,
        signature: Signature,
        hash: B256,
    ) -> Self
    where
        T: RlpEcdsaEncodableTx,
    {
        Signed::new_unchecked(transaction, signature, hash).into()
    }

    /// Creates a new signed transaction from the given typed transaction and signature without the
    /// hash.
    ///
    /// Note: this only calculates the hash on the first [`EthereumTxEnvelope::hash`] call.
    pub fn new_unhashed(transaction: EthereumTypedTransaction<T>, signature: Signature) -> Self
    where
        T: RlpEcdsaEncodableTx + SignableTransaction<Signature>,
    {
        transaction.into_signed(signature).into()
    }

    /// Consumes the type, removes the signature and returns the transaction.
    #[inline]
    pub fn into_typed_transaction(self) -> EthereumTypedTransaction<T>
    where
        T: RlpEcdsaEncodableTx,
    {
        match self {
            Self::Legacy(tx) => EthereumTypedTransaction::Legacy(tx.into_parts().0),
            Self::Eip2930(tx) => EthereumTypedTransaction::Eip2930(tx.into_parts().0),
            Self::Eip1559(tx) => EthereumTypedTransaction::Eip1559(tx.into_parts().0),
            Self::Eip4844(tx) => EthereumTypedTransaction::Eip4844(tx.into_parts().0),
            Self::Eip7702(tx) => EthereumTypedTransaction::Eip7702(tx.into_parts().0),
        }
    }

    /// Returns a mutable reference to the transaction's input.
    #[doc(hidden)]
    pub fn input_mut(&mut self) -> &mut Bytes
    where
        T: AsMut<TxEip4844>,
    {
        match self {
            Self::Eip1559(tx) => &mut tx.tx_mut().input,
            Self::Eip2930(tx) => &mut tx.tx_mut().input,
            Self::Legacy(tx) => &mut tx.tx_mut().input,
            Self::Eip7702(tx) => &mut tx.tx_mut().input,
            Self::Eip4844(tx) => &mut tx.tx_mut().as_mut().input,
        }
    }
}

impl<T> EthereumTypedTransaction<TxEip4844Variant<T>> {
    /// Strips the sidecar from EIP-4844 transactions and returns both the transaction and the
    /// sidecar separately, keeping the same sidecar type parameter.
    ///
    /// This method consumes the typed transaction and returns:
    /// - An [`EthereumTypedTransaction<TxEip4844Variant<T>>`] with the sidecar stripped from
    ///   EIP-4844 transactions
    /// - An [`Option<T>`] containing the sidecar if this was an EIP-4844 transaction with a sidecar
    ///
    /// For non-EIP-4844 transactions, this returns the transaction unchanged with `None` for the
    /// sidecar.
    ///
    /// This is a convenience wrapper around
    /// [`strip_eip4844_sidecar_into`](Self::strip_eip4844_sidecar_into) that keeps the same type
    /// parameter.
    ///
    /// # Examples
    ///
    /// ```
    /// # use alloy_consensus::{EthereumTypedTransaction, TxEip4844Variant};
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # fn example(tx: EthereumTypedTransaction<TxEip4844Variant<BlobTransactionSidecar>>) {
    /// // Strip the sidecar from the transaction (type parameter stays the same)
    /// let (tx_without_sidecar, maybe_sidecar) = tx.strip_eip4844_sidecar();
    ///
    /// if let Some(sidecar) = maybe_sidecar {
    ///     // Process the blob sidecar separately
    ///     println!("Transaction had {} blobs", sidecar.blobs.len());
    /// }
    /// # }
    /// ```
    pub fn strip_eip4844_sidecar(self) -> (Self, Option<T>) {
        self.strip_eip4844_sidecar_into()
    }

    /// Strips the sidecar from EIP-4844 transactions and returns both the transaction and the
    /// sidecar separately, converting to a different sidecar type parameter.
    ///
    /// This method consumes the typed transaction and returns:
    /// - An [`EthereumTypedTransaction<TxEip4844Variant<U>>`] with the sidecar stripped from
    ///   EIP-4844 transactions
    /// - An [`Option<T>`] containing the sidecar if this was an EIP-4844 transaction with a sidecar
    ///
    /// For non-EIP-4844 transactions, this simply converts the type parameter and returns `None`
    /// for the sidecar.
    ///
    /// This is useful when you need to:
    /// - Extract blob data from pooled transactions for separate processing
    /// - Convert between different sidecar type parameters
    /// - Prepare transactions for storage (without sidecars)
    ///
    /// # Examples
    ///
    /// ```
    /// # use alloy_consensus::{EthereumTypedTransaction, TxEip4844Variant};
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # use alloy_eips::eip7594::BlobTransactionSidecarVariant;
    /// # fn example(tx: EthereumTypedTransaction<TxEip4844Variant<BlobTransactionSidecar>>) {
    /// // Strip the sidecar and convert to a different type parameter
    /// let (tx_without_sidecar, maybe_sidecar): (
    ///     EthereumTypedTransaction<TxEip4844Variant<BlobTransactionSidecarVariant>>,
    ///     _,
    /// ) = tx.strip_eip4844_sidecar_into();
    ///
    /// if let Some(sidecar) = maybe_sidecar {
    ///     // Process the blob sidecar separately
    ///     println!("Transaction had {} blobs", sidecar.blobs.len());
    /// }
    /// # }
    /// ```
    pub fn strip_eip4844_sidecar_into<U>(
        self,
    ) -> (EthereumTypedTransaction<TxEip4844Variant<U>>, Option<T>) {
        match self {
            Self::Legacy(tx) => (EthereumTypedTransaction::Legacy(tx), None),
            Self::Eip2930(tx) => (EthereumTypedTransaction::Eip2930(tx), None),
            Self::Eip1559(tx) => (EthereumTypedTransaction::Eip1559(tx), None),
            Self::Eip4844(tx) => {
                let (tx_variant, sidecar) = tx.strip_sidecar_into();
                (EthereumTypedTransaction::Eip4844(tx_variant), sidecar)
            }
            Self::Eip7702(tx) => (EthereumTypedTransaction::Eip7702(tx), None),
        }
    }

    /// Drops the sidecar from EIP-4844 transactions and returns only the transaction, keeping the
    /// same sidecar type parameter.
    ///
    /// This is a convenience method that discards the sidecar from EIP-4844 transactions,
    /// returning only the transaction without a sidecar.
    ///
    /// This is equivalent to calling [`strip_eip4844_sidecar`](Self::strip_eip4844_sidecar) and
    /// taking only the first element of the tuple.
    ///
    /// # Examples
    ///
    /// ```
    /// # use alloy_consensus::{EthereumTypedTransaction, TxEip4844Variant};
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # fn example(tx: EthereumTypedTransaction<TxEip4844Variant<BlobTransactionSidecar>>) {
    /// // Drop the sidecar, keeping only the transaction
    /// let tx_without_sidecar = tx.drop_eip4844_sidecar();
    /// # }
    /// ```
    pub fn drop_eip4844_sidecar(self) -> Self {
        self.strip_eip4844_sidecar().0
    }

    /// Drops the sidecar from EIP-4844 transactions and returns only the transaction, converting
    /// to a different sidecar type parameter.
    ///
    /// This is a convenience method that discards the sidecar from EIP-4844 transactions,
    /// returning only the transaction without a sidecar.
    ///
    /// This is equivalent to calling
    /// [`strip_eip4844_sidecar_into`](Self::strip_eip4844_sidecar_into) and taking only the first
    /// element of the tuple.
    ///
    /// # Examples
    ///
    /// ```
    /// # use alloy_consensus::{EthereumTypedTransaction, TxEip4844Variant};
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # use alloy_eips::eip7594::BlobTransactionSidecarVariant;
    /// # fn example(tx: EthereumTypedTransaction<TxEip4844Variant<BlobTransactionSidecar>>) {
    /// // Drop the sidecar and convert to a different type parameter
    /// let tx_without_sidecar: EthereumTypedTransaction<
    ///     TxEip4844Variant<BlobTransactionSidecarVariant>,
    /// > = tx.drop_eip4844_sidecar_into();
    /// # }
    /// ```
    pub fn drop_eip4844_sidecar_into<U>(self) -> EthereumTypedTransaction<TxEip4844Variant<U>> {
        self.strip_eip4844_sidecar_into().0
    }
}

#[cfg(feature = "kzg")]
impl EthereumTxEnvelope<TxEip4844WithSidecar<alloy_eips::eip4844::BlobTransactionSidecar>> {
    /// Converts the envelope to EIP-7594 format using default KZG settings.
    ///
    /// For EIP-4844 transactions, this computes cell KZG proofs and converts the sidecar to
    /// EIP-7594 format. Non-EIP-4844 transactions are converted to the appropriate envelope type
    /// without modification.
    ///
    /// # Returns
    ///
    /// - `Ok(EthereumTxEnvelope<TxEip4844WithSidecar<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>)` - The
    ///   envelope with EIP-7594 sidecars
    /// - `Err(c_kzg::Error)` - If KZG proof computation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use alloy_consensus::EthereumTxEnvelope;
    /// # use alloy_consensus::TxEip4844WithSidecar;
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # fn example(envelope: EthereumTxEnvelope<TxEip4844WithSidecar<BlobTransactionSidecar>>) -> Result<(), c_kzg::Error> {
    /// // Convert to EIP-7594 format
    /// let eip7594_envelope = envelope.try_into_7594()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_7594(
        self,
    ) -> Result<
        EthereumTxEnvelope<
            TxEip4844WithSidecar<alloy_eips::eip7594::BlobTransactionSidecarEip7594>,
        >,
        c_kzg::Error,
    > {
        self.try_into_7594_with_settings(
            alloy_eips::eip4844::env_settings::EnvKzgSettings::Default.get(),
        )
    }

    /// Converts the envelope to EIP-7594 format using custom KZG settings.
    ///
    /// For EIP-4844 transactions, this computes cell KZG proofs and converts the sidecar to
    /// EIP-7594 format using the provided KZG settings. Non-EIP-4844 transactions are converted
    /// to the appropriate envelope type without modification.
    ///
    /// # Arguments
    ///
    /// * `settings` - The KZG settings to use for computing cell proofs
    ///
    /// # Returns
    ///
    /// - `Ok(EthereumTxEnvelope<TxEip4844WithSidecar<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>)` - The
    ///   envelope with EIP-7594 sidecars
    /// - `Err(c_kzg::Error)` - If KZG proof computation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use alloy_consensus::EthereumTxEnvelope;
    /// # use alloy_consensus::TxEip4844WithSidecar;
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # use alloy_eips::eip4844::env_settings::EnvKzgSettings;
    /// # fn example(envelope: EthereumTxEnvelope<TxEip4844WithSidecar<BlobTransactionSidecar>>) -> Result<(), c_kzg::Error> {
    /// // Load custom KZG settings
    /// let kzg_settings = EnvKzgSettings::Default.get();
    ///
    /// // Convert using custom settings
    /// let eip7594_envelope = envelope.try_into_7594_with_settings(kzg_settings)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_7594_with_settings(
        self,
        settings: &c_kzg::KzgSettings,
    ) -> Result<
        EthereumTxEnvelope<
            TxEip4844WithSidecar<alloy_eips::eip7594::BlobTransactionSidecarEip7594>,
        >,
        c_kzg::Error,
    > {
        self.try_map_eip4844(|tx| tx.try_into_7594_with_settings(settings))
    }
}

#[cfg(feature = "kzg")]
impl EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip4844::BlobTransactionSidecar>> {
    /// Converts the envelope to EIP-7594 format using default KZG settings.
    ///
    /// For EIP-4844 transactions with sidecars, this computes cell KZG proofs and converts the
    /// sidecar to EIP-7594 format. Transactions without sidecars and non-EIP-4844 transactions
    /// are converted to the appropriate envelope type without modification.
    ///
    /// # Returns
    ///
    /// - `Ok(EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>)` - The envelope
    ///   with EIP-7594 sidecars
    /// - `Err(c_kzg::Error)` - If KZG proof computation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use alloy_consensus::EthereumTxEnvelope;
    /// # use alloy_consensus::TxEip4844Variant;
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # fn example(envelope: EthereumTxEnvelope<TxEip4844Variant<BlobTransactionSidecar>>) -> Result<(), c_kzg::Error> {
    /// // Convert to EIP-7594 format
    /// let eip7594_envelope = envelope.try_into_7594()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_7594(
        self,
    ) -> Result<
        EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>,
        c_kzg::Error,
    > {
        self.try_into_7594_with_settings(
            alloy_eips::eip4844::env_settings::EnvKzgSettings::Default.get(),
        )
    }

    /// Converts the envelope to EIP-7594 format using custom KZG settings.
    ///
    /// For EIP-4844 transactions with sidecars, this computes cell KZG proofs and converts the
    /// sidecar to EIP-7594 format using the provided KZG settings. Transactions without sidecars
    /// and non-EIP-4844 transactions are converted to the appropriate envelope type without
    /// modification.
    ///
    /// # Arguments
    ///
    /// * `settings` - The KZG settings to use for computing cell proofs
    ///
    /// # Returns
    ///
    /// - `Ok(EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>)` - The envelope
    ///   with EIP-7594 sidecars
    /// - `Err(c_kzg::Error)` - If KZG proof computation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use alloy_consensus::EthereumTxEnvelope;
    /// # use alloy_consensus::TxEip4844Variant;
    /// # use alloy_eips::eip4844::BlobTransactionSidecar;
    /// # use alloy_eips::eip4844::env_settings::EnvKzgSettings;
    /// # fn example(envelope: EthereumTxEnvelope<TxEip4844Variant<BlobTransactionSidecar>>) -> Result<(), c_kzg::Error> {
    /// // Load custom KZG settings
    /// let kzg_settings = EnvKzgSettings::Default.get();
    ///
    /// // Convert using custom settings
    /// let eip7594_envelope = envelope.try_into_7594_with_settings(kzg_settings)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_7594_with_settings(
        self,
        settings: &c_kzg::KzgSettings,
    ) -> Result<
        EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>,
        c_kzg::Error,
    > {
        self.try_map_eip4844(|tx| tx.try_into_7594_with_settings(settings))
    }
}

#[cfg(feature = "kzg")]
impl TryFrom<EthereumTxEnvelope<TxEip4844WithSidecar<alloy_eips::eip4844::BlobTransactionSidecar>>>
    for EthereumTxEnvelope<TxEip4844WithSidecar<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>
{
    type Error = c_kzg::Error;

    fn try_from(
        value: EthereumTxEnvelope<
            TxEip4844WithSidecar<alloy_eips::eip4844::BlobTransactionSidecar>,
        >,
    ) -> Result<Self, Self::Error> {
        value.try_into_7594()
    }
}

#[cfg(feature = "kzg")]
impl TryFrom<EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip4844::BlobTransactionSidecar>>>
    for EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip7594::BlobTransactionSidecarEip7594>>
{
    type Error = c_kzg::Error;

    fn try_from(
        value: EthereumTxEnvelope<TxEip4844Variant<alloy_eips::eip4844::BlobTransactionSidecar>>,
    ) -> Result<Self, Self::Error> {
        value.try_into_7594()
    }
}

/// The Ethereum [EIP-2718] Transaction Envelope.
///
/// # Note:
///
/// This enum distinguishes between tagged and untagged legacy transactions, as
/// the in-protocol merkle tree may commit to EITHER 0-prefixed or raw.
/// Therefore we must ensure that encoding returns the precise byte-array that
/// was decoded, preserving the presence or absence of the `TransactionType`
/// flag.
///
/// [EIP-2718]: https://eips.ethereum.org/EIPS/eip-2718
#[derive(Clone, Debug, TransactionEnvelope)]
#[envelope(
    alloy_consensus = crate,
    tx_type_name = TxType,
    typed = EthereumTypedTransaction,
    arbitrary_cfg(feature = "arbitrary")
)]
#[doc(alias = "TransactionEnvelope")]
pub enum EthereumTxEnvelope<Eip4844> {
    /// An untagged [`TxLegacy`].
    #[envelope(ty = 0)]
    Legacy(Signed<TxLegacy>),
    /// A [`TxEip2930`] tagged with type 1.
    #[envelope(ty = 1)]
    Eip2930(Signed<TxEip2930>),
    /// A [`TxEip1559`] tagged with type 2.
    #[envelope(ty = 2)]
    Eip1559(Signed<TxEip1559>),
    /// A TxEip4844 tagged with type 3.
    /// An EIP-4844 transaction has two network representations:
    /// 1 - The transaction itself, which is a regular RLP-encoded transaction and used to retrieve
    /// historical transactions..
    ///
    /// 2 - The transaction with a sidecar, which is the form used to
    /// send transactions to the network.
    #[envelope(ty = 3)]
    Eip4844(Signed<Eip4844>),
    /// A [`TxEip7702`] tagged with type 4.
    #[envelope(ty = 4)]
    Eip7702(Signed<TxEip7702>),
}

impl<T, Eip4844> From<Signed<T>> for EthereumTxEnvelope<Eip4844>
where
    EthereumTypedTransaction<Eip4844>: From<T>,
    T: RlpEcdsaEncodableTx,
{
    fn from(v: Signed<T>) -> Self {
        let (tx, sig, hash) = v.into_parts();
        let typed = EthereumTypedTransaction::from(tx);
        match typed {
            EthereumTypedTransaction::Legacy(tx_legacy) => {
                let tx = Signed::new_unchecked(tx_legacy, sig, hash);
                Self::Legacy(tx)
            }
            EthereumTypedTransaction::Eip2930(tx_eip2930) => {
                let tx = Signed::new_unchecked(tx_eip2930, sig, hash);
                Self::Eip2930(tx)
            }
            EthereumTypedTransaction::Eip1559(tx_eip1559) => {
                let tx = Signed::new_unchecked(tx_eip1559, sig, hash);
                Self::Eip1559(tx)
            }
            EthereumTypedTransaction::Eip4844(tx_eip4844_variant) => {
                let tx = Signed::new_unchecked(tx_eip4844_variant, sig, hash);
                Self::Eip4844(tx)
            }
            EthereumTypedTransaction::Eip7702(tx_eip7702) => {
                let tx = Signed::new_unchecked(tx_eip7702, sig, hash);
                Self::Eip7702(tx)
            }
        }
    }
}

impl<Eip4844: RlpEcdsaEncodableTx> From<EthereumTxEnvelope<Eip4844>>
    for Signed<EthereumTypedTransaction<Eip4844>>
where
    EthereumTypedTransaction<Eip4844>: From<Eip4844>,
{
    fn from(value: EthereumTxEnvelope<Eip4844>) -> Self {
        value.into_signed()
    }
}

impl<Eip4844> From<(EthereumTypedTransaction<Eip4844>, Signature)> for EthereumTxEnvelope<Eip4844>
where
    Eip4844: RlpEcdsaEncodableTx + SignableTransaction<Signature>,
{
    fn from(value: (EthereumTypedTransaction<Eip4844>, Signature)) -> Self {
        value.0.into_signed(value.1).into()
    }
}

impl<T> From<EthereumTxEnvelope<TxEip4844WithSidecar<T>>> for EthereumTxEnvelope<TxEip4844> {
    fn from(value: EthereumTxEnvelope<TxEip4844WithSidecar<T>>) -> Self {
        value.map_eip4844(|eip4844| eip4844.into())
    }
}

impl<T> From<EthereumTxEnvelope<TxEip4844Variant<T>>> for EthereumTxEnvelope<TxEip4844> {
    fn from(value: EthereumTxEnvelope<TxEip4844Variant<T>>) -> Self {
        value.map_eip4844(|eip4844| eip4844.into())
    }
}

impl<T> From<EthereumTxEnvelope<TxEip4844>> for EthereumTxEnvelope<TxEip4844Variant<T>> {
    fn from(value: EthereumTxEnvelope<TxEip4844>) -> Self {
        value.map_eip4844(|eip4844| eip4844.into())
    }
}

impl<Eip4844> EthereumTxEnvelope<Eip4844> {
    /// Converts the EIP-4844 variant of this transaction with the given closure.
    ///
    /// This is intended to convert between the EIP-4844 variants, specifically for stripping away
    /// non consensus data (blob sidecar data).
    pub fn map_eip4844<U>(self, f: impl FnMut(Eip4844) -> U) -> EthereumTxEnvelope<U> {
        match self {
            Self::Legacy(tx) => EthereumTxEnvelope::Legacy(tx),
            Self::Eip2930(tx) => EthereumTxEnvelope::Eip2930(tx),
            Self::Eip1559(tx) => EthereumTxEnvelope::Eip1559(tx),
            Self::Eip4844(tx) => EthereumTxEnvelope::Eip4844(tx.map(f)),
            Self::Eip7702(tx) => EthereumTxEnvelope::Eip7702(tx),
        }
    }

    /// Converts the EIP-4844 variant of this transaction with the given closure, returning an error
    /// if the mapping fails.
    pub fn try_map_eip4844<U, E>(
        self,
        f: impl FnOnce(Eip4844) -> Result<U, E>,
    ) -> Result<EthereumTxEnvelope<U>, E> {
        match self {
            Self::Legacy(tx) => Ok(EthereumTxEnvelope::Legacy(tx)),
            Self::Eip2930(tx) => Ok(EthereumTxEnvelope::Eip2930(tx)),
            Self::Eip1559(tx) => Ok(EthereumTxEnvelope::Eip1559(tx)),
            Self::Eip4844(tx) => tx.try_map(f).map(EthereumTxEnvelope::Eip4844),
            Self::Eip7702(tx) => Ok(EthereumTxEnvelope::Eip7702(tx)),
        }
    }

    /// Return the [`TxType`] of the inner txn.
    #[doc(alias = "transaction_type")]
    pub const fn tx_type(&self) -> TxType {
        match self {
            Self::Legacy(_) => TxType::Legacy,
            Self::Eip2930(_) => TxType::Eip2930,
            Self::Eip1559(_) => TxType::Eip1559,
            Self::Eip4844(_) => TxType::Eip4844,
            Self::Eip7702(_) => TxType::Eip7702,
        }
    }

    /// Consumes the type into a [`Signed`]
    pub fn into_signed(self) -> Signed<EthereumTypedTransaction<Eip4844>>
    where
        EthereumTypedTransaction<Eip4844>: From<Eip4844>,
    {
        match self {
            Self::Legacy(tx) => tx.convert(),
            Self::Eip2930(tx) => tx.convert(),
            Self::Eip1559(tx) => tx.convert(),
            Self::Eip4844(tx) => tx.convert(),
            Self::Eip7702(tx) => tx.convert(),
        }
    }
}

impl<Eip4844: RlpEcdsaEncodableTx> EthereumTxEnvelope<Eip4844> {
    /// Returns true if the transaction is a legacy transaction.
    #[inline]
    pub const fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy(_))
    }

    /// Returns true if the transaction is an EIP-2930 transaction.
    #[inline]
    pub const fn is_eip2930(&self) -> bool {
        matches!(self, Self::Eip2930(_))
    }

    /// Returns true if the transaction is an EIP-1559 transaction.
    #[inline]
    pub const fn is_eip1559(&self) -> bool {
        matches!(self, Self::Eip1559(_))
    }

    /// Returns true if the transaction is an EIP-4844 transaction.
    #[inline]
    pub const fn is_eip4844(&self) -> bool {
        matches!(self, Self::Eip4844(_))
    }

    /// Returns true if the transaction is an EIP-7702 transaction.
    #[inline]
    pub const fn is_eip7702(&self) -> bool {
        matches!(self, Self::Eip7702(_))
    }

    /// Returns true if the transaction is replay protected.
    ///
    /// All non-legacy transactions are replay protected, as the chain id is
    /// included in the transaction body. Legacy transactions are considered
    /// replay protected if the `v` value is not 27 or 28, according to the
    /// rules of [EIP-155].
    ///
    /// [EIP-155]: https://eips.ethereum.org/EIPS/eip-155
    #[inline]
    pub const fn is_replay_protected(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.tx().chain_id.is_some(),
            _ => true,
        }
    }

    /// Returns the [`TxLegacy`] variant if the transaction is a legacy transaction.
    pub const fn as_legacy(&self) -> Option<&Signed<TxLegacy>> {
        match self {
            Self::Legacy(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip2930`] variant if the transaction is an EIP-2930 transaction.
    pub const fn as_eip2930(&self) -> Option<&Signed<TxEip2930>> {
        match self {
            Self::Eip2930(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip1559`] variant if the transaction is an EIP-1559 transaction.
    pub const fn as_eip1559(&self) -> Option<&Signed<TxEip1559>> {
        match self {
            Self::Eip1559(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip4844Variant`] variant if the transaction is an EIP-4844 transaction.
    pub const fn as_eip4844(&self) -> Option<&Signed<Eip4844>> {
        match self {
            Self::Eip4844(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip7702`] variant if the transaction is an EIP-7702 transaction.
    pub const fn as_eip7702(&self) -> Option<&Signed<TxEip7702>> {
        match self {
            Self::Eip7702(tx) => Some(tx),
            _ => None,
        }
    }

    /// Consumes the type and returns the [`TxLegacy`] variant if the transaction is a legacy
    /// transaction. Returns an error otherwise.
    pub fn try_into_legacy(self) -> Result<Signed<TxLegacy>, ValueError<Self>> {
        match self {
            Self::Legacy(tx) => Ok(tx),
            _ => Err(ValueError::new_static(self, "Expected legacy transaction")),
        }
    }

    /// Consumes the type and returns the [`TxEip2930`] variant if the transaction is an EIP-2930
    /// transaction. Returns an error otherwise.
    pub fn try_into_eip2930(self) -> Result<Signed<TxEip2930>, ValueError<Self>> {
        match self {
            Self::Eip2930(tx) => Ok(tx),
            _ => Err(ValueError::new_static(self, "Expected EIP-2930 transaction")),
        }
    }

    /// Consumes the type and returns the [`TxEip1559`] variant if the transaction is an EIP-1559
    /// transaction. Returns an error otherwise.    
    pub fn try_into_eip1559(self) -> Result<Signed<TxEip1559>, ValueError<Self>> {
        match self {
            Self::Eip1559(tx) => Ok(tx),
            _ => Err(ValueError::new_static(self, "Expected EIP-1559 transaction")),
        }
    }

    /// Consumes the type and returns the [`TxEip4844`] variant if the transaction is an EIP-4844
    /// transaction. Returns an error otherwise.
    pub fn try_into_eip4844(self) -> Result<Signed<Eip4844>, ValueError<Self>> {
        match self {
            Self::Eip4844(tx) => Ok(tx),
            _ => Err(ValueError::new_static(self, "Expected EIP-4844 transaction")),
        }
    }

    /// Consumes the type and returns the [`TxEip7702`] variant if the transaction is an EIP-7702
    /// transaction. Returns an error otherwise.
    pub fn try_into_eip7702(self) -> Result<Signed<TxEip7702>, ValueError<Self>> {
        match self {
            Self::Eip7702(tx) => Ok(tx),
            _ => Err(ValueError::new_static(self, "Expected EIP-7702 transaction")),
        }
    }

    /// Calculate the signing hash for the transaction.
    pub fn signature_hash(&self) -> B256
    where
        Eip4844: SignableTransaction<Signature>,
    {
        match self {
            Self::Legacy(tx) => tx.signature_hash(),
            Self::Eip2930(tx) => tx.signature_hash(),
            Self::Eip1559(tx) => tx.signature_hash(),
            Self::Eip4844(tx) => tx.signature_hash(),
            Self::Eip7702(tx) => tx.signature_hash(),
        }
    }

    /// Return the reference to signature.
    pub const fn signature(&self) -> &Signature {
        match self {
            Self::Legacy(tx) => tx.signature(),
            Self::Eip2930(tx) => tx.signature(),
            Self::Eip1559(tx) => tx.signature(),
            Self::Eip4844(tx) => tx.signature(),
            Self::Eip7702(tx) => tx.signature(),
        }
    }

    /// Return the hash of the inner Signed.
    #[doc(alias = "transaction_hash")]
    pub fn tx_hash(&self) -> &B256 {
        match self {
            Self::Legacy(tx) => tx.hash(),
            Self::Eip2930(tx) => tx.hash(),
            Self::Eip1559(tx) => tx.hash(),
            Self::Eip4844(tx) => tx.hash(),
            Self::Eip7702(tx) => tx.hash(),
        }
    }

    /// Reference to transaction hash. Used to identify transaction.
    pub fn hash(&self) -> &B256 {
        match self {
            Self::Legacy(tx) => tx.hash(),
            Self::Eip2930(tx) => tx.hash(),
            Self::Eip1559(tx) => tx.hash(),
            Self::Eip7702(tx) => tx.hash(),
            Self::Eip4844(tx) => tx.hash(),
        }
    }

    /// Return the length of the inner txn, including type byte length
    pub fn eip2718_encoded_length(&self) -> usize {
        match self {
            Self::Legacy(t) => t.eip2718_encoded_length(),
            Self::Eip2930(t) => t.eip2718_encoded_length(),
            Self::Eip1559(t) => t.eip2718_encoded_length(),
            Self::Eip4844(t) => t.eip2718_encoded_length(),
            Self::Eip7702(t) => t.eip2718_encoded_length(),
        }
    }
}

impl<Eip4844: RlpEcdsaEncodableTx> TxHashRef for EthereumTxEnvelope<Eip4844> {
    fn tx_hash(&self) -> &B256 {
        Self::tx_hash(self)
    }
}

#[cfg(any(feature = "secp256k1", feature = "k256"))]
impl<Eip4844> crate::transaction::SignerRecoverable for EthereumTxEnvelope<Eip4844>
where
    Eip4844: RlpEcdsaEncodableTx + SignableTransaction<Signature>,
{
    fn recover_signer(&self) -> Result<alloy_primitives::Address, crate::crypto::RecoveryError> {
        match self {
            Self::Legacy(tx) => crate::transaction::SignerRecoverable::recover_signer(tx),
            Self::Eip2930(tx) => crate::transaction::SignerRecoverable::recover_signer(tx),
            Self::Eip1559(tx) => crate::transaction::SignerRecoverable::recover_signer(tx),
            Self::Eip4844(tx) => crate::transaction::SignerRecoverable::recover_signer(tx),
            Self::Eip7702(tx) => crate::transaction::SignerRecoverable::recover_signer(tx),
        }
    }

    fn recover_signer_unchecked(
        &self,
    ) -> Result<alloy_primitives::Address, crate::crypto::RecoveryError> {
        match self {
            Self::Legacy(tx) => crate::transaction::SignerRecoverable::recover_signer_unchecked(tx),
            Self::Eip2930(tx) => {
                crate::transaction::SignerRecoverable::recover_signer_unchecked(tx)
            }
            Self::Eip1559(tx) => {
                crate::transaction::SignerRecoverable::recover_signer_unchecked(tx)
            }
            Self::Eip4844(tx) => {
                crate::transaction::SignerRecoverable::recover_signer_unchecked(tx)
            }
            Self::Eip7702(tx) => {
                crate::transaction::SignerRecoverable::recover_signer_unchecked(tx)
            }
        }
    }

    fn recover_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<alloy_primitives::Address, crate::crypto::RecoveryError> {
        match self {
            Self::Legacy(tx) => crate::transaction::SignerRecoverable::recover_with_buf(tx, buf),
            Self::Eip2930(tx) => crate::transaction::SignerRecoverable::recover_with_buf(tx, buf),
            Self::Eip1559(tx) => crate::transaction::SignerRecoverable::recover_with_buf(tx, buf),
            Self::Eip4844(tx) => crate::transaction::SignerRecoverable::recover_with_buf(tx, buf),
            Self::Eip7702(tx) => crate::transaction::SignerRecoverable::recover_with_buf(tx, buf),
        }
    }

    fn recover_unchecked_with_buf(
        &self,
        buf: &mut alloc::vec::Vec<u8>,
    ) -> Result<alloy_primitives::Address, crate::crypto::RecoveryError> {
        match self {
            Self::Legacy(tx) => {
                crate::transaction::SignerRecoverable::recover_unchecked_with_buf(tx, buf)
            }
            Self::Eip2930(tx) => {
                crate::transaction::SignerRecoverable::recover_unchecked_with_buf(tx, buf)
            }
            Self::Eip1559(tx) => {
                crate::transaction::SignerRecoverable::recover_unchecked_with_buf(tx, buf)
            }
            Self::Eip4844(tx) => {
                crate::transaction::SignerRecoverable::recover_unchecked_with_buf(tx, buf)
            }
            Self::Eip7702(tx) => {
                crate::transaction::SignerRecoverable::recover_unchecked_with_buf(tx, buf)
            }
        }
    }
}

/// Bincode-compatible [`EthereumTxEnvelope`] serde implementation.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    use crate::{EthereumTypedTransaction, Signed};
    use alloc::borrow::Cow;
    use alloy_primitives::Signature;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::EthereumTxEnvelope`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use alloy_consensus::{serde_bincode_compat, EthereumTxEnvelope};
    /// use serde::{de::DeserializeOwned, Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: Serialize + DeserializeOwned + Clone + 'static> {
    ///     #[serde_as(as = "serde_bincode_compat::EthereumTxEnvelope<'_, T>")]
    ///     receipt: EthereumTxEnvelope<T>,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct EthereumTxEnvelope<'a, Eip4844: Clone = crate::transaction::TxEip4844> {
        /// Transaction signature
        signature: Signature,
        /// bincode compatible transaction
        transaction:
            crate::serde_bincode_compat::transaction::EthereumTypedTransaction<'a, Eip4844>,
    }

    impl<'a, T: Clone> From<&'a super::EthereumTxEnvelope<T>> for EthereumTxEnvelope<'a, T> {
        fn from(value: &'a super::EthereumTxEnvelope<T>) -> Self {
            match value {
                super::EthereumTxEnvelope::Legacy(tx) => Self {
                    signature: *tx.signature(),
                    transaction:
                        crate::serde_bincode_compat::transaction::EthereumTypedTransaction::Legacy(
                            tx.tx().into(),
                        ),
                },
                super::EthereumTxEnvelope::Eip2930(tx) => Self {
                    signature: *tx.signature(),
                    transaction:
                        crate::serde_bincode_compat::transaction::EthereumTypedTransaction::Eip2930(
                            tx.tx().into(),
                        ),
                },
                super::EthereumTxEnvelope::Eip1559(tx) => Self {
                    signature: *tx.signature(),
                    transaction:
                        crate::serde_bincode_compat::transaction::EthereumTypedTransaction::Eip1559(
                            tx.tx().into(),
                        ),
                },
                super::EthereumTxEnvelope::Eip4844(tx) => Self {
                    signature: *tx.signature(),
                    transaction:
                        crate::serde_bincode_compat::transaction::EthereumTypedTransaction::Eip4844(
                            Cow::Borrowed(tx.tx()),
                        ),
                },
                super::EthereumTxEnvelope::Eip7702(tx) => Self {
                    signature: *tx.signature(),
                    transaction:
                        crate::serde_bincode_compat::transaction::EthereumTypedTransaction::Eip7702(
                            tx.tx().into(),
                        ),
                },
            }
        }
    }

    impl<'a, T: Clone> From<EthereumTxEnvelope<'a, T>> for super::EthereumTxEnvelope<T> {
        fn from(value: EthereumTxEnvelope<'a, T>) -> Self {
            let EthereumTxEnvelope { signature, transaction } = value;
            let transaction: crate::transaction::typed::EthereumTypedTransaction<T> =
                transaction.into();
            match transaction {
                EthereumTypedTransaction::Legacy(tx) => Signed::new_unhashed(tx, signature).into(),
                EthereumTypedTransaction::Eip2930(tx) => Signed::new_unhashed(tx, signature).into(),
                EthereumTypedTransaction::Eip1559(tx) => Signed::new_unhashed(tx, signature).into(),
                EthereumTypedTransaction::Eip4844(tx) => {
                    Self::Eip4844(Signed::new_unhashed(tx, signature))
                }
                EthereumTypedTransaction::Eip7702(tx) => Signed::new_unhashed(tx, signature).into(),
            }
        }
    }

    impl<T: Serialize + Clone> SerializeAs<super::EthereumTxEnvelope<T>> for EthereumTxEnvelope<'_, T> {
        fn serialize_as<S>(
            source: &super::EthereumTxEnvelope<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            EthereumTxEnvelope::<'_, T>::from(source).serialize(serializer)
        }
    }

    impl<'de, T: Deserialize<'de> + Clone> DeserializeAs<'de, super::EthereumTxEnvelope<T>>
        for EthereumTxEnvelope<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::EthereumTxEnvelope<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            EthereumTxEnvelope::<'_, T>::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, EthereumTxEnvelope};
        use crate::TxEip4844;
        use arbitrary::Arbitrary;
        use bincode::config;
        use rand::Rng;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_typed_tx_envelope_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::EthereumTxEnvelope<'_>")]
                transaction: EthereumTxEnvelope<TxEip4844>,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(bytes.as_mut_slice());
            let data = Data {
                transaction: EthereumTxEnvelope::arbitrary(&mut arbitrary::Unstructured::new(
                    &bytes,
                ))
                .unwrap(),
            };

            let encoded = bincode::serde::encode_to_vec(&data, config::legacy()).unwrap();
            let (decoded, _) =
                bincode::serde::decode_from_slice::<Data, _>(&encoded, config::legacy()).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        transaction::{Recovered, SignableTransaction, SignerRecoverable},
        Transaction, TxEip4844, TxEip4844WithSidecar,
    };
    use alloc::vec::Vec;
    use alloy_eips::{
        eip2930::{AccessList, AccessListItem},
        eip4844::BlobTransactionSidecar,
        eip7594::BlobTransactionSidecarVariant,
        eip7702::Authorization,
    };
    #[allow(unused_imports)]
    use alloy_primitives::{b256, Bytes, TxKind};
    use alloy_primitives::{hex, Address, Signature, U256};
    use alloy_rlp::Decodable;
    use std::{fs, path::PathBuf, str::FromStr, vec};

    #[test]
    fn assert_encodable() {
        fn assert_encodable<T: Encodable2718>() {}

        assert_encodable::<EthereumTxEnvelope<TxEip4844>>();
        assert_encodable::<Recovered<EthereumTxEnvelope<TxEip4844>>>();
        assert_encodable::<Recovered<EthereumTxEnvelope<TxEip4844Variant>>>();
    }

    #[test]
    #[cfg(feature = "k256")]
    // Test vector from https://etherscan.io/tx/0xce4dc6d7a7549a98ee3b071b67e970879ff51b5b95d1c340bacd80fa1e1aab31
    fn test_decode_live_1559_tx() {
        use alloy_primitives::address;

        let raw_tx = alloy_primitives::hex::decode("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8").unwrap();
        let res = TxEnvelope::decode(&mut raw_tx.as_slice()).unwrap();

        assert_eq!(res.tx_type(), TxType::Eip1559);

        let tx = match res {
            TxEnvelope::Eip1559(tx) => tx,
            _ => unreachable!(),
        };

        assert_eq!(tx.tx().to, TxKind::Call(address!("D9e1459A7A482635700cBc20BBAF52D495Ab9C96")));
        let from = tx.recover_signer().unwrap();
        assert_eq!(from, address!("001e2b7dE757bA469a57bF6b23d982458a07eFcE"));
    }

    #[test]
    fn test_is_replay_protected_v() {
        let sig = Signature::test_signature();
        assert!(!&TxEnvelope::Legacy(Signed::new_unchecked(
            TxLegacy::default(),
            sig,
            Default::default(),
        ))
        .is_replay_protected());
        let r = b256!("840cfc572845f5786e702984c2a582528cad4b49b2a10b9db1be7fca90058565");
        let s = b256!("25e7109ceb98168d95b09b18bbf6b685130e0562f233877d492b94eee0c5b6d1");
        let v = false;
        let valid_sig = Signature::from_scalars_and_parity(r, s, v);
        assert!(!&TxEnvelope::Legacy(Signed::new_unchecked(
            TxLegacy::default(),
            valid_sig,
            Default::default(),
        ))
        .is_replay_protected());
        assert!(&TxEnvelope::Eip2930(Signed::new_unchecked(
            TxEip2930::default(),
            sig,
            Default::default(),
        ))
        .is_replay_protected());
        assert!(&TxEnvelope::Eip1559(Signed::new_unchecked(
            TxEip1559::default(),
            sig,
            Default::default(),
        ))
        .is_replay_protected());
        assert!(&TxEnvelope::Eip4844(Signed::new_unchecked(
            TxEip4844Variant::TxEip4844(TxEip4844::default()),
            sig,
            Default::default(),
        ))
        .is_replay_protected());
        assert!(&TxEnvelope::Eip7702(Signed::new_unchecked(
            TxEip7702::default(),
            sig,
            Default::default(),
        ))
        .is_replay_protected());
    }

    #[test]
    #[cfg(feature = "k256")]
    // Test vector from https://etherscan.io/tx/0x280cde7cdefe4b188750e76c888f13bd05ce9a4d7767730feefe8a0e50ca6fc4
    fn test_decode_live_legacy_tx() {
        use alloy_primitives::address;

        let raw_tx = alloy_primitives::bytes!("f9015482078b8505d21dba0083022ef1947a250d5630b4cf539739df2c5dacb4c659f2488d880c46549a521b13d8b8e47ff36ab50000000000000000000000000000000000000000000066ab5a608bd00a23f2fe000000000000000000000000000000000000000000000000000000000000008000000000000000000000000048c04ed5691981c42154c6167398f95e8f38a7ff00000000000000000000000000000000000000000000000000000000632ceac70000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000006c6ee5e31d828de241282b9606c8e98ea48526e225a0c9077369501641a92ef7399ff81c21639ed4fd8fc69cb793cfa1dbfab342e10aa0615facb2f1bcf3274a354cfe384a38d0cc008a11c2dd23a69111bc6930ba27a8");
        let res = TxEnvelope::decode_2718(&mut raw_tx.as_ref()).unwrap();
        assert_eq!(res.tx_type(), TxType::Legacy);

        let tx = match res {
            TxEnvelope::Legacy(tx) => tx,
            _ => unreachable!(),
        };

        assert_eq!(tx.tx().chain_id(), Some(1));

        assert_eq!(tx.tx().to, TxKind::Call(address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D")));
        assert_eq!(
            tx.hash().to_string(),
            "0x280cde7cdefe4b188750e76c888f13bd05ce9a4d7767730feefe8a0e50ca6fc4"
        );
        let from = tx.recover_signer().unwrap();
        assert_eq!(from, address!("a12e1462d0ceD572f396F58B6E2D03894cD7C8a4"));
    }

    #[test]
    #[cfg(feature = "k256")]
    // Test vector from https://sepolia.etherscan.io/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    // Blobscan: https://sepolia.blobscan.com/tx/0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
    fn test_decode_live_4844_tx() {
        use crate::Transaction;
        use alloy_primitives::{address, b256};

        // https://sepolia.etherscan.io/getRawTx?tx=0x9a22ccb0029bc8b0ddd073be1a1d923b7ae2b2ea52100bae0db4424f9107e9c0
        let raw_tx = alloy_primitives::hex::decode("0x03f9011d83aa36a7820fa28477359400852e90edd0008252089411e9ca82a3a762b4b5bd264d4173a242e7a770648080c08504a817c800f8a5a0012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921aa00152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4a0013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7a001148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1a0011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e654901a0c8de4cced43169f9aa3d36506363b2d2c44f6c49fc1fd91ea114c86f3757077ea01e11fdd0d1934eda0492606ee0bb80a7bf8f35cc5f86ec60fe5031ba48bfd544").unwrap();

        let res = TxEnvelope::decode_2718(&mut raw_tx.as_slice()).unwrap();
        assert_eq!(res.tx_type(), TxType::Eip4844);

        let tx = match res {
            TxEnvelope::Eip4844(tx) => tx,
            _ => unreachable!(),
        };

        assert_eq!(
            tx.tx().kind(),
            TxKind::Call(address!("11E9CA82A3a762b4B5bd264d4173a242e7a77064"))
        );

        // Assert this is the correct variant of the EIP-4844 enum, which only contains the tx.
        assert!(matches!(tx.tx(), TxEip4844Variant::TxEip4844(_)));

        assert_eq!(
            tx.tx().tx().blob_versioned_hashes,
            vec![
                b256!("012ec3d6f66766bedb002a190126b3549fce0047de0d4c25cffce0dc1c57921a"),
                b256!("0152d8e24762ff22b1cfd9f8c0683786a7ca63ba49973818b3d1e9512cd2cec4"),
                b256!("013b98c6c83e066d5b14af2b85199e3d4fc7d1e778dd53130d180f5077e2d1c7"),
                b256!("01148b495d6e859114e670ca54fb6e2657f0cbae5b08063605093a4b3dc9f8f1"),
                b256!("011ac212f13c5dff2b2c6b600a79635103d6f580a4221079951181b25c7e6549")
            ]
        );

        let from = tx.recover_signer().unwrap();
        assert_eq!(from, address!("0xA83C816D4f9b2783761a22BA6FADB0eB0606D7B2"));
    }

    fn test_encode_decode_roundtrip<T: SignableTransaction<Signature>>(
        tx: T,
        signature: Option<Signature>,
    ) where
        Signed<T>: Into<TxEnvelope>,
    {
        let signature = signature.unwrap_or_else(Signature::test_signature);
        let tx_signed = tx.into_signed(signature);
        let tx_envelope: TxEnvelope = tx_signed.into();
        let encoded = tx_envelope.encoded_2718();
        let mut slice = encoded.as_slice();
        let decoded = TxEnvelope::decode_2718(&mut slice).unwrap();
        assert_eq!(encoded.len(), tx_envelope.encode_2718_len());
        assert_eq!(decoded, tx_envelope);
        assert_eq!(slice.len(), 0);
    }

    #[test]
    fn test_encode_decode_legacy() {
        let tx = TxLegacy {
            chain_id: None,
            nonce: 2,
            gas_limit: 1000000,
            gas_price: 10000000000,
            to: Address::left_padding_from(&[6]).into(),
            value: U256::from(7_u64),
            ..Default::default()
        };
        test_encode_decode_roundtrip(tx, Some(Signature::test_signature().with_parity(true)));
    }

    #[test]
    fn test_encode_decode_eip1559() {
        let tx = TxEip1559 {
            chain_id: 1u64,
            nonce: 2,
            max_fee_per_gas: 3,
            max_priority_fee_per_gas: 4,
            gas_limit: 5,
            to: Address::left_padding_from(&[6]).into(),
            value: U256::from(7_u64),
            input: vec![8].into(),
            access_list: Default::default(),
        };
        test_encode_decode_roundtrip(tx, None);
    }

    #[test]
    fn test_encode_decode_eip1559_parity_eip155() {
        let tx = TxEip1559 {
            chain_id: 1u64,
            nonce: 2,
            max_fee_per_gas: 3,
            max_priority_fee_per_gas: 4,
            gas_limit: 5,
            to: Address::left_padding_from(&[6]).into(),
            value: U256::from(7_u64),
            input: vec![8].into(),
            access_list: Default::default(),
        };
        let signature = Signature::test_signature().with_parity(true);

        test_encode_decode_roundtrip(tx, Some(signature));
    }

    #[test]
    fn test_encode_decode_eip2930_parity_eip155() {
        let tx = TxEip2930 {
            chain_id: 1u64,
            nonce: 2,
            gas_price: 3,
            gas_limit: 4,
            to: Address::left_padding_from(&[5]).into(),
            value: U256::from(6_u64),
            input: vec![7].into(),
            access_list: Default::default(),
        };
        let signature = Signature::test_signature().with_parity(true);
        test_encode_decode_roundtrip(tx, Some(signature));
    }

    #[test]
    fn test_encode_decode_eip4844_parity_eip155() {
        let tx = TxEip4844 {
            chain_id: 1,
            nonce: 100,
            max_fee_per_gas: 50_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000_000,
            gas_limit: 1_000_000,
            to: Address::random(),
            value: U256::from(10e18),
            input: Bytes::new(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::random(),
                storage_keys: vec![B256::random()],
            }]),
            blob_versioned_hashes: vec![B256::random()],
            max_fee_per_blob_gas: 0,
        };
        let signature = Signature::test_signature().with_parity(true);
        test_encode_decode_roundtrip(tx, Some(signature));
    }

    #[test]
    fn test_encode_decode_eip4844_sidecar_parity_eip155() {
        let tx = TxEip4844 {
            chain_id: 1,
            nonce: 100,
            max_fee_per_gas: 50_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000_000,
            gas_limit: 1_000_000,
            to: Address::random(),
            value: U256::from(10e18),
            input: Bytes::new(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::random(),
                storage_keys: vec![B256::random()],
            }]),
            blob_versioned_hashes: vec![B256::random()],
            max_fee_per_blob_gas: 0,
        };
        let sidecar = BlobTransactionSidecar {
            blobs: vec![[2; 131072].into()],
            commitments: vec![[3; 48].into()],
            proofs: vec![[4; 48].into()],
        };
        let tx = TxEip4844WithSidecar { tx, sidecar };
        let signature = Signature::test_signature().with_parity(true);

        let tx_signed = tx.into_signed(signature);
        let tx_envelope: TxEnvelope = tx_signed.into();

        let mut out = Vec::new();
        tx_envelope.network_encode(&mut out);
        let mut slice = out.as_slice();
        let decoded = TxEnvelope::network_decode(&mut slice).unwrap();
        assert_eq!(slice.len(), 0);
        assert_eq!(out.len(), tx_envelope.network_len());
        assert_eq!(decoded, tx_envelope);
    }

    #[test]
    fn test_encode_decode_eip4844_variant_parity_eip155() {
        let tx = TxEip4844 {
            chain_id: 1,
            nonce: 100,
            max_fee_per_gas: 50_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000_000,
            gas_limit: 1_000_000,
            to: Address::random(),
            value: U256::from(10e18),
            input: Bytes::new(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::random(),
                storage_keys: vec![B256::random()],
            }]),
            blob_versioned_hashes: vec![B256::random()],
            max_fee_per_blob_gas: 0,
        };
        let tx: TxEip4844Variant = tx.into();
        let signature = Signature::test_signature().with_parity(true);
        test_encode_decode_roundtrip(tx, Some(signature));
    }

    #[test]
    fn test_encode_decode_eip2930() {
        let tx = TxEip2930 {
            chain_id: 1u64,
            nonce: 2,
            gas_price: 3,
            gas_limit: 4,
            to: Address::left_padding_from(&[5]).into(),
            value: U256::from(6_u64),
            input: vec![7].into(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::left_padding_from(&[8]),
                storage_keys: vec![B256::left_padding_from(&[9])],
            }]),
        };
        test_encode_decode_roundtrip(tx, None);
    }

    #[test]
    fn test_encode_decode_eip7702() {
        let tx = TxEip7702 {
            chain_id: 1u64,
            nonce: 2,
            gas_limit: 3,
            max_fee_per_gas: 4,
            max_priority_fee_per_gas: 5,
            to: Address::left_padding_from(&[5]),
            value: U256::from(6_u64),
            input: vec![7].into(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::left_padding_from(&[8]),
                storage_keys: vec![B256::left_padding_from(&[9])],
            }]),
            authorization_list: vec![(Authorization {
                chain_id: U256::from(1),
                address: Address::left_padding_from(&[10]),
                nonce: 1u64,
            })
            .into_signed(Signature::from_str("48b55bfa915ac795c431978d8a6a992b628d557da5ff759b307d495a36649353efffd310ac743f371de3b9f7f9cb56c0b28ad43601b4ab949f53faa07bd2c8041b").unwrap())],
        };
        test_encode_decode_roundtrip(tx, None);
    }

    #[test]
    fn test_encode_decode_transaction_list() {
        let signature = Signature::test_signature();
        let tx = TxEnvelope::Eip1559(
            TxEip1559 {
                chain_id: 1u64,
                nonce: 2,
                max_fee_per_gas: 3,
                max_priority_fee_per_gas: 4,
                gas_limit: 5,
                to: Address::left_padding_from(&[6]).into(),
                value: U256::from(7_u64),
                input: vec![8].into(),
                access_list: Default::default(),
            }
            .into_signed(signature),
        );
        let transactions = vec![tx.clone(), tx];
        let encoded = alloy_rlp::encode(&transactions);
        let decoded = Vec::<TxEnvelope>::decode(&mut &encoded[..]).unwrap();
        assert_eq!(transactions, decoded);
    }

    #[test]
    fn decode_encode_known_rpc_transaction() {
        // test data pulled from hive test that sends blob transactions
        let network_data_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/rpc_blob_transaction.rlp");
        let data = fs::read_to_string(network_data_path).expect("Unable to read file");
        let hex_data = hex::decode(data.trim()).unwrap();

        let tx: TxEnvelope = TxEnvelope::decode_2718(&mut hex_data.as_slice()).unwrap();
        let encoded = tx.encoded_2718();
        assert_eq!(encoded, hex_data);
        assert_eq!(tx.encode_2718_len(), hex_data.len());
    }

    #[cfg(feature = "serde")]
    fn test_serde_roundtrip<T: SignableTransaction<Signature>>(tx: T)
    where
        Signed<T>: Into<TxEnvelope>,
    {
        let signature = Signature::test_signature();
        let tx_envelope: TxEnvelope = tx.into_signed(signature).into();

        let serialized = serde_json::to_string(&tx_envelope).unwrap();

        let deserialized: TxEnvelope = serde_json::from_str(&serialized).unwrap();

        assert_eq!(tx_envelope, deserialized);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_roundtrip_legacy() {
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 100,
            gas_price: 3_000_000_000,
            gas_limit: 50_000,
            to: Address::default().into(),
            value: U256::from(10e18),
            input: Bytes::new(),
        };
        test_serde_roundtrip(tx);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_roundtrip_eip1559() {
        let tx = TxEip1559 {
            chain_id: 1,
            nonce: 100,
            max_fee_per_gas: 50_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000_000,
            gas_limit: 1_000_000,
            to: TxKind::Create,
            value: U256::from(10e18),
            input: Bytes::new(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::random(),
                storage_keys: vec![B256::random()],
            }]),
        };
        test_serde_roundtrip(tx);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_roundtrip_eip2930() {
        let tx = TxEip2930 {
            chain_id: u64::MAX,
            nonce: u64::MAX,
            gas_price: u128::MAX,
            gas_limit: u64::MAX,
            to: Address::random().into(),
            value: U256::MAX,
            input: Bytes::new(),
            access_list: Default::default(),
        };
        test_serde_roundtrip(tx);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_roundtrip_eip4844() {
        let tx: TxEip4844Variant = TxEip4844 {
            chain_id: 1,
            nonce: 100,
            max_fee_per_gas: 50_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000_000,
            gas_limit: 1_000_000,
            to: Address::random(),
            value: U256::from(10e18),
            input: Bytes::new(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::random(),
                storage_keys: vec![B256::random()],
            }]),
            blob_versioned_hashes: vec![B256::random()],
            max_fee_per_blob_gas: 0,
        }
        .into();
        test_serde_roundtrip(tx);

        let tx = TxEip4844Variant::TxEip4844WithSidecar(TxEip4844WithSidecar {
            tx: TxEip4844 {
                chain_id: 1,
                nonce: 100,
                max_fee_per_gas: 50_000_000_000,
                max_priority_fee_per_gas: 1_000_000_000_000,
                gas_limit: 1_000_000,
                to: Address::random(),
                value: U256::from(10e18),
                input: Bytes::new(),
                access_list: AccessList(vec![AccessListItem {
                    address: Address::random(),
                    storage_keys: vec![B256::random()],
                }]),
                blob_versioned_hashes: vec![B256::random()],
                max_fee_per_blob_gas: 0,
            },
            sidecar: BlobTransactionSidecarVariant::Eip4844(Default::default()),
        });
        test_serde_roundtrip(tx);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_roundtrip_eip7702() {
        let tx = TxEip7702 {
            chain_id: u64::MAX,
            nonce: u64::MAX,
            gas_limit: u64::MAX,
            max_fee_per_gas: u128::MAX,
            max_priority_fee_per_gas: u128::MAX,
            to: Address::random(),
            value: U256::MAX,
            input: Bytes::new(),
            access_list: AccessList(vec![AccessListItem {
                address: Address::random(),
                storage_keys: vec![B256::random()],
            }]),
            authorization_list: vec![(Authorization {
                chain_id: U256::from(1),
                address: Address::left_padding_from(&[1]),
                nonce: 1u64,
            })
            .into_signed(Signature::from_str("48b55bfa915ac795c431978d8a6a992b628d557da5ff759b307d495a36649353efffd310ac743f371de3b9f7f9cb56c0b28ad43601b4ab949f53faa07bd2c8041b").unwrap())],
        };
        test_serde_roundtrip(tx);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn serde_tx_from_contract_call() {
        let rpc_tx = r#"{"hash":"0x018b2331d461a4aeedf6a1f9cc37463377578244e6a35216057a8370714e798f","nonce":"0x1","blockHash":"0x3ca295f1dcaf8ac073c543dc0eccf18859f411206df181731e374e9917252931","blockNumber":"0x2","transactionIndex":"0x0","from":"0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266","to":"0x5fbdb2315678afecb367f032d93f642f64180aa3","value":"0x0","gasPrice":"0x3a29f0f8","gas":"0x1c9c380","maxFeePerGas":"0xba43b7400","maxPriorityFeePerGas":"0x5f5e100","input":"0xd09de08a","r":"0xd309309a59a49021281cb6bb41d164c96eab4e50f0c1bd24c03ca336e7bc2bb7","s":"0x28a7f089143d0a1355ebeb2a1b9f0e5ad9eca4303021c1400d61bc23c9ac5319","v":"0x0","yParity":"0x0","chainId":"0x7a69","accessList":[],"type":"0x2"}"#;

        let te = serde_json::from_str::<TxEnvelope>(rpc_tx).unwrap();

        assert_eq!(
            *te.tx_hash(),
            alloy_primitives::b256!(
                "018b2331d461a4aeedf6a1f9cc37463377578244e6a35216057a8370714e798f"
            )
        );
    }

    #[test]
    #[cfg(feature = "k256")]
    fn test_arbitrary_envelope() {
        use crate::transaction::SignerRecoverable;
        use arbitrary::Arbitrary;
        let mut unstructured = arbitrary::Unstructured::new(b"arbitrary tx envelope");
        let tx = TxEnvelope::arbitrary(&mut unstructured).unwrap();

        assert!(tx.recover_signer().is_ok());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_untagged_legacy() {
        let data = r#"{
            "hash": "0x97efb58d2b42df8d68ab5899ff42b16c7e0af35ed86ae4adb8acaad7e444220c",
            "input": "0x",
            "r": "0x5d71a4a548503f2916d10c6b1a1557a0e7352eb041acb2bac99d1ad6bb49fd45",
            "s": "0x2627bf6d35be48b0e56c61733f63944c0ebcaa85cb4ed6bc7cba3161ba85e0e8",
            "v": "0x1c",
            "gas": "0x15f90",
            "from": "0x2a65aca4d5fc5b5c859090a6c34d164135398226",
            "to": "0x8fbeb4488a08d60979b5aa9e13dd00b2726320b2",
            "value": "0xf606682badd7800",
            "nonce": "0x11f398",
            "gasPrice": "0x4a817c800"
        }"#;

        let tx: TxEnvelope = serde_json::from_str(data).unwrap();

        assert!(matches!(tx, TxEnvelope::Legacy(_)));

        let data_with_wrong_type = r#"{
            "hash": "0x97efb58d2b42df8d68ab5899ff42b16c7e0af35ed86ae4adb8acaad7e444220c",
            "input": "0x",
            "r": "0x5d71a4a548503f2916d10c6b1a1557a0e7352eb041acb2bac99d1ad6bb49fd45",
            "s": "0x2627bf6d35be48b0e56c61733f63944c0ebcaa85cb4ed6bc7cba3161ba85e0e8",
            "v": "0x1c",
            "gas": "0x15f90",
            "from": "0x2a65aca4d5fc5b5c859090a6c34d164135398226",
            "to": "0x8fbeb4488a08d60979b5aa9e13dd00b2726320b2",
            "value": "0xf606682badd7800",
            "nonce": "0x11f398",
            "gasPrice": "0x4a817c800",
            "type": "0x12"
        }"#;

        assert!(serde_json::from_str::<TxEnvelope>(data_with_wrong_type).is_err());
    }

    #[test]
    fn test_tx_type_try_from_u8() {
        assert_eq!(TxType::try_from(0u8).unwrap(), TxType::Legacy);
        assert_eq!(TxType::try_from(1u8).unwrap(), TxType::Eip2930);
        assert_eq!(TxType::try_from(2u8).unwrap(), TxType::Eip1559);
        assert_eq!(TxType::try_from(3u8).unwrap(), TxType::Eip4844);
        assert_eq!(TxType::try_from(4u8).unwrap(), TxType::Eip7702);
        assert!(TxType::try_from(5u8).is_err()); // Invalid case
    }

    #[test]
    fn test_tx_type_try_from_u64() {
        assert_eq!(TxType::try_from(0u64).unwrap(), TxType::Legacy);
        assert_eq!(TxType::try_from(1u64).unwrap(), TxType::Eip2930);
        assert_eq!(TxType::try_from(2u64).unwrap(), TxType::Eip1559);
        assert_eq!(TxType::try_from(3u64).unwrap(), TxType::Eip4844);
        assert_eq!(TxType::try_from(4u64).unwrap(), TxType::Eip7702);
        assert!(TxType::try_from(10u64).is_err()); // Invalid case
    }

    #[test]
    fn test_tx_type_from_conversions() {
        let legacy_tx = Signed::new_unchecked(
            TxLegacy::default(),
            Signature::test_signature(),
            Default::default(),
        );
        let eip2930_tx = Signed::new_unchecked(
            TxEip2930::default(),
            Signature::test_signature(),
            Default::default(),
        );
        let eip1559_tx = Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            Default::default(),
        );
        let eip4844_variant = Signed::new_unchecked(
            TxEip4844Variant::<BlobTransactionSidecarVariant>::TxEip4844(TxEip4844::default()),
            Signature::test_signature(),
            Default::default(),
        );
        let eip7702_tx = Signed::new_unchecked(
            TxEip7702::default(),
            Signature::test_signature(),
            Default::default(),
        );

        assert!(matches!(TxEnvelope::from(legacy_tx), TxEnvelope::Legacy(_)));
        assert!(matches!(TxEnvelope::from(eip2930_tx), TxEnvelope::Eip2930(_)));
        assert!(matches!(TxEnvelope::from(eip1559_tx), TxEnvelope::Eip1559(_)));
        assert!(matches!(TxEnvelope::from(eip4844_variant), TxEnvelope::Eip4844(_)));
        assert!(matches!(TxEnvelope::from(eip7702_tx), TxEnvelope::Eip7702(_)));
    }

    #[test]
    fn test_tx_type_is_methods() {
        let legacy_tx = TxEnvelope::Legacy(Signed::new_unchecked(
            TxLegacy::default(),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip2930_tx = TxEnvelope::Eip2930(Signed::new_unchecked(
            TxEip2930::default(),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip1559_tx = TxEnvelope::Eip1559(Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip4844_tx = TxEnvelope::Eip4844(Signed::new_unchecked(
            TxEip4844Variant::TxEip4844(TxEip4844::default()),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip7702_tx = TxEnvelope::Eip7702(Signed::new_unchecked(
            TxEip7702::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        assert!(legacy_tx.is_legacy());
        assert!(!legacy_tx.is_eip2930());
        assert!(!legacy_tx.is_eip1559());
        assert!(!legacy_tx.is_eip4844());
        assert!(!legacy_tx.is_eip7702());

        assert!(eip2930_tx.is_eip2930());
        assert!(!eip2930_tx.is_legacy());
        assert!(!eip2930_tx.is_eip1559());
        assert!(!eip2930_tx.is_eip4844());
        assert!(!eip2930_tx.is_eip7702());

        assert!(eip1559_tx.is_eip1559());
        assert!(!eip1559_tx.is_legacy());
        assert!(!eip1559_tx.is_eip2930());
        assert!(!eip1559_tx.is_eip4844());
        assert!(!eip1559_tx.is_eip7702());

        assert!(eip4844_tx.is_eip4844());
        assert!(!eip4844_tx.is_legacy());
        assert!(!eip4844_tx.is_eip2930());
        assert!(!eip4844_tx.is_eip1559());
        assert!(!eip4844_tx.is_eip7702());

        assert!(eip7702_tx.is_eip7702());
        assert!(!eip7702_tx.is_legacy());
        assert!(!eip7702_tx.is_eip2930());
        assert!(!eip7702_tx.is_eip1559());
        assert!(!eip7702_tx.is_eip4844());
    }

    #[test]
    fn test_tx_type() {
        let legacy_tx = TxEnvelope::Legacy(Signed::new_unchecked(
            TxLegacy::default(),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip2930_tx = TxEnvelope::Eip2930(Signed::new_unchecked(
            TxEip2930::default(),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip1559_tx = TxEnvelope::Eip1559(Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip4844_tx = TxEnvelope::Eip4844(Signed::new_unchecked(
            TxEip4844Variant::TxEip4844(TxEip4844::default()),
            Signature::test_signature(),
            Default::default(),
        ));
        let eip7702_tx = TxEnvelope::Eip7702(Signed::new_unchecked(
            TxEip7702::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        assert_eq!(legacy_tx.tx_type(), TxType::Legacy);
        assert_eq!(eip2930_tx.tx_type(), TxType::Eip2930);
        assert_eq!(eip1559_tx.tx_type(), TxType::Eip1559);
        assert_eq!(eip4844_tx.tx_type(), TxType::Eip4844);
        assert_eq!(eip7702_tx.tx_type(), TxType::Eip7702);
    }

    #[test]
    fn test_try_into_legacy_success() {
        let legacy_tx = TxEnvelope::Legacy(Signed::new_unchecked(
            TxLegacy::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = legacy_tx.try_into_legacy();
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_into_legacy_failure() {
        let eip1559_tx = TxEnvelope::Eip1559(Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip1559_tx.try_into_legacy();
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected legacy transaction"));
        // Test that we can recover the original envelope
        let recovered_envelope = error.into_value();
        assert!(recovered_envelope.is_eip1559());
    }

    #[test]
    fn test_try_into_eip2930_success() {
        let eip2930_tx = TxEnvelope::Eip2930(Signed::new_unchecked(
            TxEip2930::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip2930_tx.try_into_eip2930();
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_into_eip2930_failure() {
        let legacy_tx = TxEnvelope::Legacy(Signed::new_unchecked(
            TxLegacy::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = legacy_tx.try_into_eip2930();
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected EIP-2930 transaction"));
        let recovered_envelope = error.into_value();
        assert!(recovered_envelope.is_legacy());
    }

    #[test]
    fn test_try_into_eip1559_success() {
        let eip1559_tx = TxEnvelope::Eip1559(Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip1559_tx.try_into_eip1559();
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_into_eip1559_failure() {
        let eip2930_tx = TxEnvelope::Eip2930(Signed::new_unchecked(
            TxEip2930::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip2930_tx.try_into_eip1559();
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected EIP-1559 transaction"));
        let recovered_envelope = error.into_value();
        assert!(recovered_envelope.is_eip2930());
    }

    #[test]
    fn test_try_into_eip4844_success() {
        let eip4844_tx = TxEnvelope::Eip4844(Signed::new_unchecked(
            TxEip4844Variant::TxEip4844(TxEip4844::default()),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip4844_tx.try_into_eip4844();
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_into_eip4844_failure() {
        let eip1559_tx = TxEnvelope::Eip1559(Signed::new_unchecked(
            TxEip1559::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip1559_tx.try_into_eip4844();
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected EIP-4844 transaction"));
        let recovered_envelope = error.into_value();
        assert!(recovered_envelope.is_eip1559());
    }

    #[test]
    fn test_try_into_eip7702_success() {
        let eip7702_tx = TxEnvelope::Eip7702(Signed::new_unchecked(
            TxEip7702::default(),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip7702_tx.try_into_eip7702();
        assert!(result.is_ok());
    }

    #[test]
    fn test_try_into_eip7702_failure() {
        let eip4844_tx = TxEnvelope::Eip4844(Signed::new_unchecked(
            TxEip4844Variant::TxEip4844(TxEip4844::default()),
            Signature::test_signature(),
            Default::default(),
        ));

        let result = eip4844_tx.try_into_eip7702();
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Expected EIP-7702 transaction"));
        let recovered_envelope = error.into_value();
        assert!(recovered_envelope.is_eip4844());
    }

    // <https://sepolia.etherscan.io/getRawTx?tx=0xe5b458ba9de30b47cb7c0ea836bec7b072053123a7416c5082c97f959a4eebd6>
    #[test]
    fn decode_raw_legacy() {
        let raw = hex!("f8aa0285018ef61d0a832dc6c094cb33aa5b38d79e3d9fa8b10aff38aa201399a7e380b844af7b421018842e4628f3d9ee0e2c7679e29ed5dbaa75be75efecd392943503c9c68adce800000000000000000000000000000000000000000000000000000000000000641ca05e28679806caa50d25e9cb16aef8c0c08b235241b8f6e9d86faadf70421ba664a02353bba82ef2c7ce4dd6695942399163160000272b14f9aa6cbadf011b76efa4");
        let tx = TxEnvelope::decode_2718(&mut raw.as_ref()).unwrap();
        assert!(tx.chain_id().is_none());
    }

    #[test]
    #[cfg(feature = "serde")]
    fn can_deserialize_system_transaction_with_zero_signature_envelope() {
        let raw_tx = r#"{
            "blockHash": "0x5307b5c812a067f8bc1ed1cc89d319ae6f9a0c9693848bd25c36b5191de60b85",
            "blockNumber": "0x45a59bb",
            "from": "0x0000000000000000000000000000000000000000",
            "gas": "0x1e8480",
            "gasPrice": "0x0",
            "hash": "0x16ef68aa8f35add3a03167a12b5d1268e344f6605a64ecc3f1c3aa68e98e4e06",
            "input": "0xcbd4ece900000000000000000000000032155c9d39084f040ba17890fe8134dbe2a0453f0000000000000000000000004a0126ee88018393b1ad2455060bc350ead9908a000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000469f700000000000000000000000000000000000000000000000000000000000000644ff746f60000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000002043e908a4e862aebb10e7e27db0b892b58a7e32af11d64387a414dabc327b00e200000000000000000000000000000000000000000000000000000000",
            "nonce": "0x469f7",
            "to": "0x4200000000000000000000000000000000000007",
            "transactionIndex": "0x0",
            "value": "0x0",
            "v": "0x0",
            "r": "0x0",
            "s": "0x0",
            "queueOrigin": "l1",
            "l1TxOrigin": "0x36bde71c97b33cc4729cf772ae268934f7ab70b2",
            "l1BlockNumber": "0xfd1a6c",
            "l1Timestamp": "0x63e434ff",
            "index": "0x45a59ba",
            "queueIndex": "0x469f7",
            "rawTransaction": "0xcbd4ece900000000000000000000000032155c9d39084f040ba17890fe8134dbe2a0453f0000000000000000000000004a0126ee88018393b1ad2455060bc350ead9908a000000000000000000000000000000000000000000000000000000000000008000000000000000000000000000000000000000000000000000000000000469f700000000000000000000000000000000000000000000000000000000000000644ff746f60000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000002043e908a4e862aebb10e7e27db0b892b58a7e32af11d64387a414dabc327b00e200000000000000000000000000000000000000000000000000000000"
        }"#;

        let tx = serde_json::from_str::<TxEnvelope>(raw_tx).unwrap();

        assert_eq!(tx.signature().r(), U256::ZERO);
        assert_eq!(tx.signature().s(), U256::ZERO);
        assert!(!tx.signature().v());

        assert_eq!(
            tx.hash(),
            &b256!("0x16ef68aa8f35add3a03167a12b5d1268e344f6605a64ecc3f1c3aa68e98e4e06"),
            "hash should match the transaction hash"
        );
    }

    // <https://github.com/succinctlabs/kona/issues/31>
    #[test]
    #[cfg(feature = "serde")]
    fn serde_block_tx() {
        let rpc_tx = r#"{
      "blockHash": "0xc0c3190292a82c2ee148774e37e5665f6a205f5ef0cd0885e84701d90ebd442e",
      "blockNumber": "0x6edcde",
      "transactionIndex": "0x7",
      "hash": "0x2cb125e083d6d2631e3752bd2b3d757bf31bf02bfe21de0ffa46fbb118d28b19",
      "from": "0x03e5badf3bb1ade1a8f33f94536c827b6531948d",
      "to": "0x3267e72dc8780a1512fa69da7759ec66f30350e3",
      "input": "0x62e4c545000000000000000000000000464c8ec100f2f42fb4e42e07e203da2324f9fc6700000000000000000000000003e5badf3bb1ade1a8f33f94536c827b6531948d000000000000000000000000a064bfb5c7e81426647dc20a0d854da1538559dc00000000000000000000000000000000000000000000000000c6f3b40b6c0000",
      "nonce": "0x2a8",
      "value": "0x0",
      "gas": "0x28afd",
      "gasPrice": "0x23ec5dbc2",
      "accessList": [],
      "chainId": "0xaa36a7",
      "type": "0x0",
      "v": "0x1546d71",
      "r": "0x809b9f0a1777e376cd1ee5d2f551035643755edf26ea65b7a00c822a24504962",
      "s": "0x6a57bb8e21fe85c7e092868ee976fef71edca974d8c452fcf303f9180c764f64"
    }"#;

        let _ = serde_json::from_str::<TxEnvelope>(rpc_tx).unwrap();
    }

    // <https://github.com/succinctlabs/kona/issues/31>
    #[test]
    #[cfg(feature = "serde")]
    fn serde_block_tx_legacy_chain_id() {
        let rpc_tx = r#"{
      "blockHash": "0xc0c3190292a82c2ee148774e37e5665f6a205f5ef0cd0885e84701d90ebd442e",
      "blockNumber": "0x6edcde",
      "transactionIndex": "0x8",
      "hash": "0xe5b458ba9de30b47cb7c0ea836bec7b072053123a7416c5082c97f959a4eebd6",
      "from": "0x8b87f0a788cc14b4f0f374da59920f5017ff05de",
      "to": "0xcb33aa5b38d79e3d9fa8b10aff38aa201399a7e3",
      "input": "0xaf7b421018842e4628f3d9ee0e2c7679e29ed5dbaa75be75efecd392943503c9c68adce80000000000000000000000000000000000000000000000000000000000000064",
      "nonce": "0x2",
      "value": "0x0",
      "gas": "0x2dc6c0",
      "gasPrice": "0x18ef61d0a",
      "accessList": [],
      "chainId": "0xaa36a7",
      "type": "0x0",
      "v": "0x1c",
      "r": "0x5e28679806caa50d25e9cb16aef8c0c08b235241b8f6e9d86faadf70421ba664",
      "s": "0x2353bba82ef2c7ce4dd6695942399163160000272b14f9aa6cbadf011b76efa4"
    }"#;

        let _ = serde_json::from_str::<TxEnvelope>(rpc_tx).unwrap();
    }

    #[test]
    #[cfg(feature = "k256")]
    fn test_recover_with_buf_eip1559() {
        use alloy_primitives::address;

        // Test vector from https://etherscan.io/tx/0xce4dc6d7a7549a98ee3b071b67e970879ff51b5b95d1c340bacd80fa1e1aab31
        let raw_tx = alloy_primitives::hex::decode("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8").unwrap();
        let tx = TxEnvelope::decode(&mut raw_tx.as_slice()).unwrap();

        // Recover using the standard method
        let from_standard = tx.recover_signer().unwrap();
        assert_eq!(from_standard, address!("001e2b7dE757bA469a57bF6b23d982458a07eFcE"));

        // Recover using the buffer method
        let mut buf = alloc::vec::Vec::new();
        let from_with_buf = tx.recover_with_buf(&mut buf).unwrap();
        assert_eq!(from_with_buf, from_standard);

        // Verify buffer was used (should contain encoded data after recovery)
        assert!(!buf.is_empty());

        // Test that reusing the buffer works correctly
        buf.clear();
        buf.extend_from_slice(b"some garbage data that should be cleared");
        let from_with_buf_reuse = tx.recover_with_buf(&mut buf).unwrap();
        assert_eq!(from_with_buf_reuse, from_standard);
    }

    #[test]
    #[cfg(feature = "k256")]
    fn test_recover_unchecked_with_buf_legacy() {
        use alloy_primitives::address;

        // Test vector from https://etherscan.io/tx/0x280cde7cdefe4b188750e76c888f13bd05ce9a4d7767730feefe8a0e50ca6fc4
        let raw_tx = alloy_primitives::bytes!("f9015482078b8505d21dba0083022ef1947a250d5630b4cf539739df2c5dacb4c659f2488d880c46549a521b13d8b8e47ff36ab50000000000000000000000000000000000000000000066ab5a608bd00a23f2fe000000000000000000000000000000000000000000000000000000000000008000000000000000000000000048c04ed5691981c42154c6167398f95e8f38a7ff00000000000000000000000000000000000000000000000000000000632ceac70000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000006c6ee5e31d828de241282b9606c8e98ea48526e225a0c9077369501641a92ef7399ff81c21639ed4fd8fc69cb793cfa1dbfab342e10aa0615facb2f1bcf3274a354cfe384a38d0cc008a11c2dd23a69111bc6930ba27a8");
        let tx = TxEnvelope::decode_2718(&mut raw_tx.as_ref()).unwrap();

        // Recover using the standard unchecked method
        let from_standard = tx.recover_signer_unchecked().unwrap();
        assert_eq!(from_standard, address!("a12e1462d0ceD572f396F58B6E2D03894cD7C8a4"));

        // Recover using the buffer unchecked method
        let mut buf = alloc::vec::Vec::new();
        let from_with_buf = tx.recover_unchecked_with_buf(&mut buf).unwrap();
        assert_eq!(from_with_buf, from_standard);

        // Verify buffer was used
        assert!(!buf.is_empty());

        // Test that buffer is properly cleared and reused
        let original_len = buf.len();
        buf.extend_from_slice(&[0xFF; 100]); // Add garbage
        let from_with_buf_reuse = tx.recover_unchecked_with_buf(&mut buf).unwrap();
        assert_eq!(from_with_buf_reuse, from_standard);
        // Buffer should be cleared and refilled with encoded data
        assert_eq!(buf.len(), original_len);
    }

    #[test]
    #[cfg(feature = "k256")]
    fn test_recover_with_buf_multiple_tx_types() {
        use alloy_primitives::address;

        // Legacy tx
        let raw_legacy = alloy_primitives::bytes!("f9015482078b8505d21dba0083022ef1947a250d5630b4cf539739df2c5dacb4c659f2488d880c46549a521b13d8b8e47ff36ab50000000000000000000000000000000000000000000066ab5a608bd00a23f2fe000000000000000000000000000000000000000000000000000000000000008000000000000000000000000048c04ed5691981c42154c6167398f95e8f38a7ff00000000000000000000000000000000000000000000000000000000632ceac70000000000000000000000000000000000000000000000000000000000000002000000000000000000000000c02aaa39b223fe8d0a0e5c4f27ead9083c756cc20000000000000000000000006c6ee5e31d828de241282b9606c8e98ea48526e225a0c9077369501641a92ef7399ff81c21639ed4fd8fc69cb793cfa1dbfab342e10aa0615facb2f1bcf3274a354cfe384a38d0cc008a11c2dd23a69111bc6930ba27a8");
        let tx_legacy = TxEnvelope::decode_2718(&mut raw_legacy.as_ref()).unwrap();

        // EIP-1559 tx
        let raw_eip1559 = alloy_primitives::hex::decode("02f86f0102843b9aca0085029e7822d68298f094d9e1459a7a482635700cbc20bbaf52d495ab9c9680841b55ba3ac080a0c199674fcb29f353693dd779c017823b954b3c69dffa3cd6b2a6ff7888798039a028ca912de909e7e6cdef9cdcaf24c54dd8c1032946dfa1d85c206b32a9064fe8").unwrap();
        let tx_eip1559 = TxEnvelope::decode(&mut raw_eip1559.as_slice()).unwrap();

        // Use a single buffer for both recoveries
        let mut buf = alloc::vec::Vec::new();

        let from_legacy = tx_legacy.recover_with_buf(&mut buf).unwrap();
        assert_eq!(from_legacy, address!("a12e1462d0ceD572f396F58B6E2D03894cD7C8a4"));

        let from_eip1559 = tx_eip1559.recover_with_buf(&mut buf).unwrap();
        assert_eq!(from_eip1559, address!("001e2b7dE757bA469a57bF6b23d982458a07eFcE"));

        // Verify that the buffer was properly reused (no allocation needed between calls)
        assert!(!buf.is_empty());
    }
}
