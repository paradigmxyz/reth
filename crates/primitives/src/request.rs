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

    /// Push a new vector of [requests](Request) into the [Requests] collection.
    pub fn push(&mut self, requests: Vec<Request>) {
        self.request_vec.push(requests);
    }
}
