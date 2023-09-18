use http::{HeaderMap, Response};

mod auth_layer;
mod jwt_secret;
mod jwt_validator;
pub use auth_layer::AuthLayer;
pub use jwt_secret::{Claims, JwtError, JwtSecret};
pub use jwt_validator::JwtAuthValidator;

/// General purpose trait to validate Http Authorization headers. It's supposed to be integrated as
/// a validator trait into an [`AuthLayer`].
pub trait AuthValidator {
    /// Body type of the error response
    type ResponseBody;

    /// This function is invoked by the [`AuthLayer`] to perform validation on Http headers.
    /// The result conveys validation errors in the form of an Http response.
    fn validate(&self, headers: &HeaderMap) -> Result<(), Response<Self::ResponseBody>>;
}
