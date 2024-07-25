//! Layer implementations used in RPC

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use http::HeaderMap;
use jsonrpsee_http_client::HttpResponse;

mod auth_client_layer;
mod auth_layer;
mod jwt_validator;

pub use auth_layer::{AuthService, ResponseFuture};

// Export alloy JWT types
pub use alloy_rpc_types_engine::{Claims, JwtError, JwtSecret};

pub use auth_client_layer::{secret_to_bearer_header, AuthClientLayer, AuthClientService};
pub use auth_layer::AuthLayer;
pub use jwt_validator::JwtAuthValidator;

/// General purpose trait to validate Http Authorization headers. It's supposed to be integrated as
/// a validator trait into an [`AuthLayer`].
pub trait AuthValidator {
    /// This function is invoked by the [`AuthLayer`] to perform validation on Http headers.
    /// The result conveys validation errors in the form of an Http response.
    fn validate(&self, headers: &HeaderMap) -> Result<(), HttpResponse>;
}
