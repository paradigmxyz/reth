//! Sync trees

use crate::{
    error::{LookupError, LookupResult, ParseDnsEntryError, ParseEntryResult},
    resolver::Resolver,
    sync::ResolveKind,
    tree::{DnsEntry, LinkEntry, TreeRootEntry},
};
use enr::{Enr, EnrKeyUnambiguous};
use secp256k1::SecretKey;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};

/// The `QueryPool` provides an aggregate state machine for driving queries to completion.
pub(crate) struct QueryPool<R: Resolver, K: EnrKeyUnambiguous> {
    /// The [Resolver] that's used to lookup queries.
    resolver: Arc<R>,
    /// All active queries
    queries: Vec<Query<K>>,
    /// buffered results
    queued_outcomes: VecDeque<QueryOutcome<K>>,
    // TODO(mattsse): add ratelimiting support
}

// === impl QueryPool ===

impl<R: Resolver, K: EnrKeyUnambiguous> QueryPool<R, K> {
    /// Resolves the root the link's domain references
    pub(crate) fn resolve_root(&mut self, link: LinkEntry<K>) {
        let resolver = Arc::clone(&self.resolver);
        self.queries.push(Query::Root(Box::pin(async move { resolve_root(resolver, link).await })))
    }

    /// Resolves the [DnsEntry] for `<hash.domain>`
    pub(crate) fn resolve_entry(&mut self, link: LinkEntry<K>, hash: String, kind: ResolveKind) {
        let resolver = Arc::clone(&self.resolver);
        self.queries.push(Query::Entry(Box::pin(async move {
            resolve_entry(resolver, link, hash, kind).await
        })))
    }

    /// Advances the state of the queries
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<QueryOutcome<K>> {
        loop {
            // drain buffered events first
            if let Some(event) = self.queued_outcomes.pop_front() {
                return Poll::Ready(event)
            }

            // advance all queries
            for idx in (0..self.queries.len()).rev() {
                let mut query = self.queries.swap_remove(idx);
                if let Poll::Ready(outcome) = query.poll(cx) {
                    self.queued_outcomes.push_back(outcome);
                } else {
                    // still pending
                    self.queries.push(query);
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
    pub(crate) entry: Option<ParseEntryResult<DnsEntry<K>>>,
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
) -> ResolveEntryResult<K> {
    let fqn = format!("{hash}.{}", link.domain);
    let mut resp = ResolveEntryResult { entry: None, link, hash, kind };
    if let Some(entry) = resolver.lookup_txt(&fqn).await {
        resp.entry = Some(entry.parse());
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
) -> ResolveRootResult<K> {
    let root = if let Some(root) = resolver.lookup_txt(&link.domain).await {
        root
    } else {
        return Err(link)
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
