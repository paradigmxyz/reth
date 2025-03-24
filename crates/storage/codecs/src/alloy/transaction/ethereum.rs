use crate::{Compact, Vec};
use alloy_consensus::{
    transaction::RlpEcdsaEncodableTx, EthereumTxEnvelope, Signed, Transaction, TxEip1559,
    TxEip2930, TxEip7702, TxLegacy, TxType,
};
use alloy_primitives::PrimitiveSignature;
use bytes::{Buf, BufMut};

/// A trait for extracting transaction without type and signature and serializing it using
/// [`Compact`] encoding.
///
/// It is not a responsibility of this trait to encode transaction type and signature. Likely this
/// will be a part of a serialization scenario with a greater scope where these values are
/// serialized separately.
///
/// See [`ToTxCompact::to_tx_compact`].
pub(super) trait ToTxCompact {
    /// Serializes inner transaction using [`Compact`] encoding. Writes the result into `buf`.
    ///
    /// The written bytes do not contain signature and transaction type. This information be needs
    /// to be serialized extra if needed.
    fn to_tx_compact(&self, buf: &mut (impl BufMut + AsMut<[u8]>));
}

/// A trait for deserializing transaction without type and signature using [`Compact`] encoding.
///
/// It is not a responsibility of this trait to extract transaction type and signature, but both
/// are needed to create the value. While these values can come from anywhere, likely this will be
/// a part of a deserialization scenario with a greater scope where these values are deserialized
/// separately.
///
/// See [`FromTxCompact::from_tx_compact`].
pub(super) trait FromTxCompact {
    type TxType;

    /// Deserializes inner transaction using [`Compact`] encoding. The concrete type is determined
    /// by `tx_type`. The `signature` is added to create typed and signed transaction.
    ///
    /// Returns a tuple of 2 elements. The first element is the deserialized value and the second
    /// is a byte slice created from `buf` with a starting position advanced by the exact amount
    /// of bytes consumed for this process.  
    fn from_tx_compact(
        buf: &[u8],
        tx_type: Self::TxType,
        signature: PrimitiveSignature,
    ) -> (Self, &[u8])
    where
        Self: Sized;
}

impl<Eip4844: Compact + Transaction> ToTxCompact for EthereumTxEnvelope<Eip4844> {
    fn to_tx_compact(&self, buf: &mut (impl BufMut + AsMut<[u8]>)) {
        match self {
            Self::Legacy(tx) => tx.tx().to_compact(buf),
            Self::Eip2930(tx) => tx.tx().to_compact(buf),
            Self::Eip1559(tx) => tx.tx().to_compact(buf),
            Self::Eip4844(tx) => tx.tx().to_compact(buf),
            Self::Eip7702(tx) => tx.tx().to_compact(buf),
        };
    }
}

impl<Eip4844: Compact + Transaction> FromTxCompact for EthereumTxEnvelope<Eip4844> {
    type TxType = TxType;

    fn from_tx_compact(
        buf: &[u8],
        tx_type: TxType,
        signature: PrimitiveSignature,
    ) -> (Self, &[u8]) {
        match tx_type {
            TxType::Legacy => {
                let (tx, buf) = TxLegacy::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Legacy(tx), buf)
            }
            TxType::Eip2930 => {
                let (tx, buf) = TxEip2930::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip2930(tx), buf)
            }
            TxType::Eip1559 => {
                let (tx, buf) = TxEip1559::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip1559(tx), buf)
            }
            TxType::Eip4844 => {
                let (tx, buf) = Eip4844::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip4844(tx), buf)
            }
            TxType::Eip7702 => {
                let (tx, buf) = TxEip7702::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (Self::Eip7702(tx), buf)
            }
        }
    }
}

pub(super) trait Envelope: FromTxCompact<TxType: Compact> {
    fn signature(&self) -> &PrimitiveSignature;
    fn tx_type(&self) -> Self::TxType;
}

impl<Eip4844: Compact + Transaction + RlpEcdsaEncodableTx> Envelope
    for EthereumTxEnvelope<Eip4844>
{
    fn signature(&self) -> &PrimitiveSignature {
        Self::signature(self)
    }

    fn tx_type(&self) -> Self::TxType {
        Self::tx_type(self)
    }
}

pub(super) trait CompactEnvelope: Sized {
    /// Takes a buffer which can be written to. *Ideally*, it returns the length written to.
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>;

    /// Takes a buffer which can be read from. Returns the object and `buf` with its internal cursor
    /// advanced (eg.`.advance(len)`).
    ///
    /// `len` can either be the `buf` remaining length, or the length of the compacted type.
    ///
    /// It will panic, if `len` is smaller than `buf.len()`.
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]);
}

impl<T: Envelope + ToTxCompact + Transaction + Send + Sync> CompactEnvelope for T {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        let start = buf.as_mut().len();

        // Placeholder for bitflags.
        // The first byte uses 4 bits as flags: IsCompressed[1bit], TxType[2bits], Signature[1bit]
        buf.put_u8(0);

        let sig_bit = self.signature().to_compact(buf) as u8;
        let zstd_bit = self.input().len() >= 32;
        let tx_bits = self.tx_type().to_compact(buf) as u8;
        let flags = sig_bit | (tx_bits << 1) | ((zstd_bit as u8) << 3);

        buf.as_mut()[start] = flags;

        if zstd_bit {
            let mut tx_buf = Vec::with_capacity(256);

            self.to_tx_compact(&mut tx_buf);

            buf.put_slice(
                &{
                    #[cfg(feature = "std")]
                    {
                        reth_zstd_compressors::TRANSACTION_COMPRESSOR.with(|compressor| {
                            let mut compressor = compressor.borrow_mut();
                            compressor.compress(&tx_buf)
                        })
                    }
                    #[cfg(not(feature = "std"))]
                    {
                        let mut compressor = reth_zstd_compressors::create_tx_compressor();
                        compressor.compress(&tx_buf)
                    }
                }
                .expect("Failed to compress"),
            );
        } else {
            self.to_tx_compact(buf);
        };

        buf.as_mut().len() - start
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let flags = buf.get_u8() as usize;

        let sig_bit = flags & 1;
        let tx_bits = (flags & 0b110) >> 1;
        let zstd_bit = flags >> 3;

        let (signature, buf) = PrimitiveSignature::from_compact(buf, sig_bit);
        let (tx_type, buf) = T::TxType::from_compact(buf, tx_bits);

        let (transaction, buf) = if zstd_bit != 0 {
            #[cfg(feature = "std")]
            {
                reth_zstd_compressors::TRANSACTION_DECOMPRESSOR.with(|decompressor| {
                    let mut decompressor = decompressor.borrow_mut();

                    let (tx, _) =
                        Self::from_tx_compact(decompressor.decompress(buf), tx_type, signature);

                    (tx, buf)
                })
            }
            #[cfg(not(feature = "std"))]
            {
                let mut decompressor = reth_zstd_compressors::create_tx_decompressor();

                let (tx, _) =
                    Self::from_tx_compact(decompressor.decompress(buf), tx_type, signature);

                (tx, buf)
            }
        } else {
            Self::from_tx_compact(buf, tx_type, signature)
        };

        (transaction, buf)
    }
}

impl<Eip4844: Compact + RlpEcdsaEncodableTx + Transaction + Send + Sync> Compact
    for EthereumTxEnvelope<Eip4844>
{
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        <Self as CompactEnvelope>::to_compact(self, buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        <Self as CompactEnvelope>::from_compact(buf, len)
    }
}
