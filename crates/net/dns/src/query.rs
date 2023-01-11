//! Sync trees

use crate::{
    error::{LookupError, LookupResult},
    resolver::Resolver,
    sync::ResolveKind,
    tree::{DnsEntry, LinkEntry, TreeRootEntry},
};
use enr::EnrKeyUnambiguous;

use std::{
    collections::VecDeque,
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Duration,
};
use std::time::Instant;
use tokio::time::Sleep;


/// The `QueryPool` provides an aggregate state machine for driving queries to completion.
pub(crate) struct QueryPool<R: Resolver, K: EnrKeyUnambiguous> {
    /// The [Resolver] that's used to lookup queries.
    resolver: Arc<R>,
    /// Buffered queries
    queued_queries: VecDeque<Query<K>>,
    /// All active queries
    active_queries: Vec<Query<K>>,
    /// buffered results
    queued_outcomes: VecDeque<QueryOutcome<K>>,
    /// Max requests per sec
    max_requests_per_sec: NonZeroUsize,
    /// Timeout for DNS lookups.
    lookup_timeout: Duration,
}

// === impl QueryPool ===

impl<R: Resolver, K: EnrKeyUnambiguous> QueryPool<R, K> {
    pub(crate) fn new(
        resolver: Arc<R>,
        max_requests_per_sec: NonZeroUsize,
        lookup_timeout: Duration,
    ) -> Self {
        Self {
            resolver,
            queued_queries:  Default::default(),
            active_queries: vec![],
            queued_outcomes: Default::default(),
            max_requests_per_sec,
            lookup_timeout,
        }
    }

    /// Resolves the root the link's domain references
    pub(crate) fn resolve_root(&mut self, link: LinkEntry<K>) {
        let resolver = Arc::clone(&self.resolver);
        let timeout = self.lookup_timeout;
        self.queued_queries
            .push(Query::Root(Box::pin(async move { resolve_root(resolver, link, timeout).await })))
    }

    /// Resolves the [DnsEntry] for `<hash.domain>`
    pub(crate) fn resolve_entry(&mut self, link: LinkEntry<K>, hash: String, kind: ResolveKind) {
        let resolver = Arc::clone(&self.resolver);
        let timeout = self.lookup_timeout;
        self.queued_queries.push(Query::Entry(Box::pin(async move {
            resolve_entry(resolver, link, hash, kind, timeout).await
        })))
    }

    /// Advances the state of the queries
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<QueryOutcome<K>> {
        loop {
            // drain buffered events first
            if let Some(event) = self.queued_outcomes.pop_front() {
                return Poll::Ready(event)
            }

            // queue in new requests

            // advance all queries
            for idx in (0..self.active_queries.len()).rev() {
                let mut query = self.active_queries.swap_remove(idx);
                if let Poll::Ready(outcome) = query.poll(cx) {
                    self.queued_outcomes.push_back(outcome);
                } else {
                    // still pending
                    self.active_queries.push(query);
                }
            }

            if self.queued_outcomes.is_empty() {
                return Poll::Pending
            }
        }
    }
}

// === Various future/type alias ===

pub(crate) struct ResolveEntryResult<K: EnrKeyUnambiguous> {
    pub(crate) entry: Option<LookupResult<DnsEntry<K>, K>>,
    pub(crate) link: LinkEntry<K>,
    pub(crate) hash: String,
    pub(crate) kind: ResolveKind,
}

pub(crate) type ResolveRootResult<K> =
    Result<LookupResult<(TreeRootEntry, LinkEntry<K>), K>, LinkEntry<K>>;

type ResolveRootFuture<K> = Pin<Box<dyn Future<Output = ResolveRootResult<K>> + Send>>;

type ResolveEntryFuture<K> = Pin<Box<dyn Future<Output = ResolveEntryResult<K>> + Send>>;

enum Query<K: EnrKeyUnambiguous> {
    Root(ResolveRootFuture<K>),
    Entry(ResolveEntryFuture<K>),
}

// === impl Query ===

impl<K: EnrKeyUnambiguous> Query<K> {
    /// Advances the query
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<QueryOutcome<K>> {
        match self {
            Query::Root(ref mut query) => {
                let outcome = ready!(query.as_mut().poll(cx));
                Poll::Ready(QueryOutcome::Root(outcome))
            }
            Query::Entry(ref mut query) => {
                let outcome = ready!(query.as_mut().poll(cx));
                Poll::Ready(QueryOutcome::Entry(outcome))
            }
        }
    }
}

/// The output the queries return
pub(crate) enum QueryOutcome<K: EnrKeyUnambiguous> {
    Root(ResolveRootResult<K>),
    Entry(ResolveEntryResult<K>),
}

/// Retrieves the [DnsEntry]
async fn resolve_entry<K: EnrKeyUnambiguous, R: Resolver>(
    resolver: Arc<R>,
    link: LinkEntry<K>,
    hash: String,
    kind: ResolveKind,
    timeout: Duration,
) -> ResolveEntryResult<K> {
    let fqn = format!("{hash}.{}", link.domain);
    let mut resp = ResolveEntryResult { entry: None, link, hash, kind };
    match lookup_with_timeout::<R, K>(&resolver, &fqn, timeout).await {
        Ok(Some(entry)) => resp.entry = Some(entry.parse::<DnsEntry<K>>().map_err(|err| err.into())),
        Err(err) => resp.entry = Some(Err(err)),
        Ok(None) => {}
    }
    resp
}

/// Retrieves the root entry the link points to and returns the verified entry
///
/// Returns an error if the record could be retrieved but is not a root entry or failed to be
/// verified.
async fn resolve_root<K: EnrKeyUnambiguous, R: Resolver>(
    resolver: Arc<R>,
    link: LinkEntry<K>,
    timeout: Duration,
) -> ResolveRootResult<K> {
    let root = match lookup_with_timeout::<R, K>(&resolver, &link.domain, timeout).await {
        Ok(Some(root)) => root,
        Ok(_) => return Err(link),
        Err(err) => return Ok(Err(err)),
    };

    match root.parse::<TreeRootEntry>() {
        Ok(root) => {
            if root.verify::<K>(&link.pubkey) {
                Ok(Ok((root, link)))
            } else {
                Ok(Err(LookupError::InvalidRoot(root, link)))
            }
        }
        Err(err) => Ok(Err(err.into())),
    }
}

async fn lookup_with_timeout<R: Resolver, K: EnrKeyUnambiguous>(
    r: &R,
    query: &str,
    timeout: Duration,
) -> LookupResult<Option<String>, K> {
    match tokio::time::timeout(timeout, r.lookup_txt(query)).await {
        Ok(res) => Ok(res),
        Err(_) => Err(LookupError::RequestTimedOut),
    }
}
