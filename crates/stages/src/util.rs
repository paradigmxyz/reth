pub(crate) mod opt {
    use async_trait::async_trait;
    use tokio::sync::mpsc::{error::SendError, Sender};

    /// Get an [Option] with the maximum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn max<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::max(v, b)))
    }

    /// Get an [Option] with the minimum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn min<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::min(v, b)))
    }

    /// An extension trait for `Option<tokio::sync::mpsc::Sender<T>>`.
    #[async_trait]
    pub(crate) trait OptSenderExt<T> {
        /// Send the given message if the `Option` contains a [Sender].
        async fn maybe_send(&self, msg: T) -> Result<(), SendError<T>>;
    }

    #[async_trait]
    impl<T> OptSenderExt<T> for Option<Sender<T>>
    where
        T: Send,
    {
        async fn maybe_send(&self, msg: T) -> Result<(), SendError<T>> {
            if let Some(rx) = &self {
                rx.send(msg).await?;
            }

            Ok(())
        }
    }
}
