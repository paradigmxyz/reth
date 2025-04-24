use crate::EraMeta;
use futures_util::{FutureExt, Stream};
use std::{
    fmt::{Debug, Formatter},
    future::Future,
    path::Path,
    pin::Pin,
    str::FromStr,
    task::{ready, Context, Poll},
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
    pub fn new(dir: impl AsRef<Path> + Send + Sync + 'static) -> Self {
        Self {
            entries: Vec::new(),
            loaded: false,
            list: Box::pin(async move {
                let mut dir = tokio::fs::read_dir(dir).await?;
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

                entries.sort_by(|(left, _), (right, _)| right.cmp(left));

                Ok(entries)
            }),
        }
    }
}

impl Stream for EraLocalDirectoryStream {
    type Item = eyre::Result<EraLocalMeta>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.loaded {
            self.entries = ready!(self.list.poll_unpin(cx))?;
            self.loaded = true;
        }

        Poll::Ready(Ok(self.entries.pop().map(|(_, v)| EraLocalMeta::new(v))).transpose())
    }
}

/// Contains information about an ERA file that is on the local file-system and is read-only.
#[derive(Debug)]
pub struct EraLocalMeta {
    path: Box<Path>,
}

impl EraLocalMeta {
    const fn new(path: Box<Path>) -> Self {
        Self { path }
    }
}

impl<T: AsRef<Path>> PartialEq<T> for EraLocalMeta {
    fn eq(&self, other: &T) -> bool {
        self.as_ref().eq(other.as_ref())
    }
}

impl AsRef<Path> for EraLocalMeta {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

impl EraMeta for EraLocalMeta {
    /// A no-op.
    fn mark_as_processed(self) -> eyre::Result<()> {
        Ok(())
    }
}
