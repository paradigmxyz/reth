use crate::{execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor, TxEnvFor};
use alloy_consensus::transaction::Either;
use alloy_evm::{block::ExecutableTxParts, RecoveredTx};
use rayon::prelude::*;
use reth_primitives_traits::TxTy;

/// [`ConfigureEvm`] extension providing methods for executing payloads.
pub trait ConfigureEngineEvm<ExecutionData>: ConfigureEvm {
    /// Returns an [`crate::EvmEnv`] for the given payload.
    fn evm_env_for_payload(&self, payload: &ExecutionData) -> Result<EvmEnvFor<Self>, Self::Error>;

    /// Returns an [`ExecutionCtxFor`] for the given payload.
    fn context_for_payload<'a>(
        &self,
        payload: &'a ExecutionData,
    ) -> Result<ExecutionCtxFor<'a, Self>, Self::Error>;

    /// Returns an [`ExecutableTxIterator`] for the given payload.
    fn tx_iterator_for_payload(
        &self,
        payload: &ExecutionData,
    ) -> Result<impl ExecutableTxIterator<Self>, Self::Error>;
}

/// Converts a raw transaction into an executable transaction.
///
/// This trait abstracts the conversion logic (e.g., decoding, signature recovery) that is
/// parallelized in the engine.
pub trait ConvertTx<RawTx>: Send + Sync + 'static {
    /// The executable transaction type.
    type Tx;
    /// Errors that may occur during conversion.
    type Error;
    /// Converts a raw transaction.
    fn convert(&self, raw: RawTx) -> Result<Self::Tx, Self::Error>;
}

// Blanket impl so closures still work.
impl<F, RawTx, Tx, Err> ConvertTx<RawTx> for F
where
    F: Fn(RawTx) -> Result<Tx, Err> + Send + Sync + 'static,
{
    type Tx = Tx;
    type Error = Err;
    fn convert(&self, raw: RawTx) -> Result<Tx, Err> {
        self(raw)
    }
}

impl<A, B, RA, RB> ConvertTx<Either<RA, RB>> for Either<A, B>
where
    A: ConvertTx<RA>,
    B: ConvertTx<RB>,
{
    type Tx = Either<A::Tx, B::Tx>;
    type Error = Either<A::Error, B::Error>;
    fn convert(&self, raw: Either<RA, RB>) -> Result<Self::Tx, Self::Error> {
        match (self, raw) {
            (Self::Left(a), Either::Left(raw)) => {
                a.convert(raw).map(Either::Left).map_err(Either::Left)
            }
            (Self::Right(b), Either::Right(raw)) => {
                b.convert(raw).map(Either::Right).map_err(Either::Right)
            }
            _ => unreachable!(),
        }
    }
}

/// A helper trait representing a pair of a "raw" transactions iterator and a closure that can be
/// used to convert them to an executable transaction. This tuple is used in the engine to
/// parallelize heavy work like decoding or recovery.
pub trait ExecutableTxTuple: Send + 'static {
    /// Raw transaction that can be converted to an [`ExecutableTxTuple::Tx`]
    ///
    /// This can be any type that can be converted to an [`ExecutableTxTuple::Tx`]. For example,
    /// an unrecovered transaction or just the transaction bytes.
    type RawTx: Send + Sync + 'static;
    /// The executable transaction type iterator yields.
    type Tx: Clone + Send + Sync + 'static;
    /// Errors that may occur while recovering or decoding transactions.
    type Error: core::error::Error + Send + Sync + 'static;

    /// Iterator over [`ExecutableTxTuple::Tx`].
    type IntoIter: IntoParallelIterator<Item = Self::RawTx, Iter: IndexedParallelIterator>
        + IntoIterator<Item = Self::RawTx>
        + Send
        + 'static;
    /// Converter that can be used to convert a [`ExecutableTxTuple::RawTx`] to a
    /// [`ExecutableTxTuple::Tx`]. This might involve heavy work like decoding or recovery
    /// and will be parallelized in the engine.
    type Convert: ConvertTx<Self::RawTx, Tx = Self::Tx, Error = Self::Error>;

    /// Decomposes into the raw transaction iterator and converter.
    fn into_parts(self) -> (Self::IntoIter, Self::Convert);

    /// Splits off the first `n` raw transactions and returns them together with the remaining
    /// transactions and converter.
    ///
    /// Used by the engine to recover a small prefix sequentially before handing the tail to
    /// rayon, avoiding a full `collect()` of the remaining items.
    fn split_prefix(self, n: usize) -> (Vec<Self::RawTx>, Vec<Self::RawTx>, Self::Convert)
    where
        Self: Sized,
    {
        let (iter, convert) = self.into_parts();
        let mut head: Vec<Self::RawTx> = iter.into_iter().collect();
        let tail = head.split_off(n.min(head.len()));
        (head, tail, convert)
    }
}

impl<RawTx, Tx, Err, I, F> ExecutableTxTuple for (I, F)
where
    RawTx: Send + Sync + 'static,
    Tx: Clone + Send + Sync + 'static,
    Err: core::error::Error + Send + Sync + 'static,
    I: IntoParallelIterator<Item = RawTx, Iter: IndexedParallelIterator>
        + IntoIterator<Item = RawTx>
        + Send
        + 'static,
    F: Fn(RawTx) -> Result<Tx, Err> + Send + Sync + 'static,
{
    type RawTx = RawTx;
    type Tx = Tx;
    type Error = Err;

    type IntoIter = I;
    type Convert = F;

    fn into_parts(self) -> (I, F) {
        self
    }
}

/// Iterator over executable transactions.
pub trait ExecutableTxIterator<Evm: ConfigureEvm>:
    ExecutableTxTuple<Tx: ExecutableTxFor<Evm, Recovered = Self::Recovered>>
{
    /// HACK: for some reason, this duplicated AT is the only way to enforce the inner Recovered:
    /// Send + Sync bound. Effectively alias for `Self::Tx::Recovered`.
    type Recovered: RecoveredTx<TxTy<Evm::Primitives>> + Send + Sync;
}

impl<T, Evm: ConfigureEvm> ExecutableTxIterator<Evm> for T
where
    T: ExecutableTxTuple<Tx: ExecutableTxFor<Evm, Recovered: Send + Sync>>,
{
    type Recovered = <T::Tx as ExecutableTxParts<TxEnvFor<Evm>, TxTy<Evm::Primitives>>>::Recovered;
}

/// Wraps `Either<L, R>` to implement both [`IntoParallelIterator`] and [`IntoIterator`],
/// mapping items through [`Either::Left`] / [`Either::Right`] on demand without collecting.
#[derive(Debug)]
pub struct EitherIter<L, R>(Either<L, R>);

impl<L, R> IntoParallelIterator for EitherIter<L, R>
where
    L: IntoParallelIterator,
    R: IntoParallelIterator,
    L::Iter: IndexedParallelIterator,
    R::Iter: IndexedParallelIterator,
{
    type Item = Either<L::Item, R::Item>;
    type Iter = Either<
        rayon::iter::Map<L::Iter, fn(L::Item) -> Either<L::Item, R::Item>>,
        rayon::iter::Map<R::Iter, fn(R::Item) -> Either<L::Item, R::Item>>,
    >;

    fn into_par_iter(self) -> Self::Iter {
        match self.0 {
            Either::Left(l) => Either::Left(l.into_par_iter().map(Either::Left)),
            Either::Right(r) => Either::Right(r.into_par_iter().map(Either::Right)),
        }
    }
}

impl<L, R> IntoIterator for EitherIter<L, R>
where
    L: IntoIterator,
    R: IntoIterator,
{
    type Item = Either<L::Item, R::Item>;
    type IntoIter = Either<
        core::iter::Map<L::IntoIter, fn(L::Item) -> Either<L::Item, R::Item>>,
        core::iter::Map<R::IntoIter, fn(R::Item) -> Either<L::Item, R::Item>>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        match self.0 {
            Either::Left(l) => Either::Left(l.into_iter().map(Either::Left)),
            Either::Right(r) => Either::Right(r.into_iter().map(Either::Right)),
        }
    }
}

// SAFETY: `EitherIter` is just a newtype over `Either<L, R>`.
unsafe impl<L: Send, R: Send> Send for EitherIter<L, R> {}

impl<A: ExecutableTxTuple, B: ExecutableTxTuple> ExecutableTxTuple for Either<A, B> {
    type RawTx = Either<A::RawTx, B::RawTx>;
    type Tx = Either<A::Tx, B::Tx>;
    type Error = Either<A::Error, B::Error>;
    type IntoIter = EitherIter<A::IntoIter, B::IntoIter>;
    type Convert = Either<A::Convert, B::Convert>;

    fn into_parts(self) -> (Self::IntoIter, Self::Convert) {
        match self {
            Self::Left(a) => {
                let (iter, convert) = a.into_parts();
                (EitherIter(Either::Left(iter)), Either::Left(convert))
            }
            Self::Right(b) => {
                let (iter, convert) = b.into_parts();
                (EitherIter(Either::Right(iter)), Either::Right(convert))
            }
        }
    }

    fn split_prefix(self, n: usize) -> (Vec<Self::RawTx>, Vec<Self::RawTx>, Self::Convert) {
        match self {
            Self::Left(a) => {
                let (head, tail, convert) = a.split_prefix(n);
                (
                    head.into_iter().map(Either::Left).collect(),
                    tail.into_iter().map(Either::Left).collect(),
                    Either::Left(convert),
                )
            }
            Self::Right(b) => {
                let (head, tail, convert) = b.split_prefix(n);
                (
                    head.into_iter().map(Either::Right).collect(),
                    tail.into_iter().map(Either::Right).collect(),
                    Either::Right(convert),
                )
            }
        }
    }
}
