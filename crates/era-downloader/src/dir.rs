use futures_util::{FutureExt, Stream};
use std::{
    fmt::{Debug, Formatter},
    future::Future,
    path::Path,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};

type ListFuture =
    Pin<Box<dyn Future<Output = eyre::Result<Vec<(usize, Box<Path>)>>> + Send + Sync + 'static>>;

/// An ordered asynchronous [`Stream`] of ERA1 files from a directory on the local file-system.
pub struct EraLocalDirectoryStream {
    entries: Vec<(usize, Box<Path>)>,
    loaded: bool,
    list: ListFuture,
}

impl Debug for EraLocalDirectoryStream {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.loaded {
            write!(f, "EraLocalDirectoryStream({:?})", self.entries)
        } else {
            write!(f, "EraLocalDirectoryStream")
        }
    }
}

impl EraLocalDirectoryStream {
    /// Creates a new [`EraLocalDirectoryStream`] reading files from `dir`.
    pub fn new(dir: Box<Path>) -> Self {
        Self {
            entries: Vec::new(),
            loaded: false,
            list: Box::pin(async move {
                let mut dir = tokio::fs::read_dir(&dir).await?;
                let mut entries = vec![];

                while let Some(entry) = dir.next_entry().await? {
                    let path = entry.path();

                    if path.extension() != Some("era1".as_ref()) {
                        continue;
                    }

                    if let Some(last) = path.components().next_back() {
                        let str = last.as_os_str().to_string_lossy().to_string();
                        let parts = str.split('-').collect::<Vec<_>>();

                        if parts.len() == 3 {
                            let number = usize::from_str(parts[1])?;
                            entries.push((number, path.into_boxed_path()));
                        }
                    }
                }

                entries.sort_by(|(left, _), (right, _)| left.cmp(right));
                entries.reverse();

                Ok(entries)
            }),
        }
    }
}

impl Stream for EraLocalDirectoryStream {
    type Item = eyre::Result<Box<Path>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.loaded {
            match self.list.poll_unpin(cx) {
                Poll::Ready(entries) => {
                    self.entries = entries?;
                    self.loaded = true;
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        Poll::Ready(Ok(self.entries.pop().map(|(_, v)| v)).transpose())
    }
}
