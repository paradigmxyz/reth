pub(crate) mod opt {
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

    /// The producing side of a [tokio::mpsc] channel that may or may not be set.
    #[derive(Default, Clone)]
    pub(crate) struct MaybeSender<T> {
        inner: Option<Sender<T>>,
    }

    impl<T> MaybeSender<T> {
        /// Create a new [MaybeSender]
        pub(crate) fn new(sender: Option<Sender<T>>) -> Self {
            Self { inner: sender }
        }

        /// Send a value over the channel if an internal sender has been set.
        pub(crate) async fn send(&self, value: T) -> Result<(), SendError<T>> {
            if let Some(rx) = &self.inner {
                rx.send(value).await
            } else {
                Ok(())
            }
        }

        /// Set or unset the internal sender.
        pub(crate) fn set(&mut self, sender: Option<Sender<T>>) {
            self.inner = sender;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opt_max() {
            assert_eq!(max(None, 5), Some(5));
            assert_eq!(max(Some(1), 5), Some(5));
            assert_eq!(max(Some(10), 5), Some(10));
        }

        #[test]
        fn opt_min() {
            assert_eq!(min(None, 5), Some(5));
            assert_eq!(min(Some(1), 5), Some(1));
            assert_eq!(min(Some(10), 5), Some(5));
        }
    }
}
