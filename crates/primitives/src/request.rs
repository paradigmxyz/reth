use std::ops::{Deref, DerefMut};

use alloy_consensus::Request;

/// A collection of requests organized as a two-dimensional vector.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Requests {
    /// A two-dimensional vector of [Request] instances.
    pub request_vec: Vec<Vec<Request>>,
}

impl Requests {
    /// Create a new [Requests] instance with an empty vector.
    pub fn new() -> Self {
        Self { request_vec: vec![] }
    }

    /// Create a new [Requests] instance from an existing vector.
    pub fn from_vec(vec: Vec<Vec<Request>>) -> Self {
        Self { request_vec: vec }
    }

    /// Returns the length of the [Requests] vector.
    pub fn len(&self) -> usize {
        self.request_vec.len()
    }

    /// Returns `true` if the [Requests] vector is empty.
    pub fn is_empty(&self) -> bool {
        self.request_vec.is_empty()
    }

    /// Push a new vector of [requests](Request) into the [Requests] collection.
    pub fn push(&mut self, requests: Vec<Request>) {
        self.request_vec.push(requests);
    }
}

impl Deref for Requests {
    type Target = Vec<Vec<Request>>;

    fn deref(&self) -> &Self::Target {
        &self.request_vec
    }
}

impl DerefMut for Requests {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.request_vec
    }
}

impl IntoIterator for Requests {
    type Item = Vec<Request>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.request_vec.into_iter()
    }
}

impl FromIterator<Vec<Request>> for Requests {
    fn from_iter<I: IntoIterator<Item = Vec<Request>>>(iter: I) -> Self {
        Self::from_vec(iter.into_iter().collect())
    }
}
