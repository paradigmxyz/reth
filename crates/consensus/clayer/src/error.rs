use serde::{Deserialize, Serialize};

/// Represents an error received from a remote procecdure call.
#[derive(Debug, Serialize, Deserialize)]
pub enum RpcError {
    NoResultField,
    Eip155Error,
    InvalidJson(String),
    Error(String),
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpcError::NoResultField => write!(f, "No result field in response"),
            RpcError::Eip155Error => write!(f, "Not synced past EIP-155"),
            RpcError::InvalidJson(e) => write!(f, "Malformed JSON received: {}", e),
            RpcError::Error(s) => write!(f, "{}", s),
        }
    }
}

impl From<RpcError> for String {
    fn from(e: RpcError) -> String {
        e.to_string()
    }
}

use std::error::Error as StdError;
use std::fmt;

pub struct PrettyReqwestError(reqwest::Error);

impl PrettyReqwestError {
    pub fn inner(&self) -> &reqwest::Error {
        &self.0
    }
}

impl fmt::Debug for PrettyReqwestError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(url) = self.0.url() {
            write!(f, "url: {}", url)?;
        }

        let kind = if self.0.is_builder() {
            "builder"
        } else if self.0.is_redirect() {
            "redirect"
        } else if self.0.is_status() {
            "status"
        } else if self.0.is_timeout() {
            "timeout"
        } else if self.0.is_request() {
            "request"
        } else if self.0.is_connect() {
            "connect"
        } else if self.0.is_body() {
            "body"
        } else if self.0.is_decode() {
            "decode"
        } else {
            "unknown"
        };
        write!(f, ", kind: {}", kind)?;

        if let Some(status) = self.0.status() {
            write!(f, ", status_code: {}", status)?;
        }

        if let Some(ref source) = self.0.source() {
            write!(f, ", detail: {}", source)?;
        } else {
            write!(f, ", source: unknown")?;
        }

        Ok(())
    }
}

impl fmt::Display for PrettyReqwestError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl From<reqwest::Error> for PrettyReqwestError {
    fn from(inner: reqwest::Error) -> Self {
        Self(inner)
    }
}
