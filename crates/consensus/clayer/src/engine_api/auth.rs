use std::path::PathBuf;

use jsonwebtoken::{encode, get_current_timestamp, Algorithm, EncodingKey, Header};
use rand::Rng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Default algorithm used for JWT token signing.
const DEFAULT_ALGORITHM: Algorithm = Algorithm::HS256;

/// JWT secret length in bytes.
pub const JWT_SECRET_LENGTH: usize = 32;

#[derive(Debug)]
pub enum Error {
    JWT(jsonwebtoken::errors::Error),
    InvalidToken,
    InvalidKey(String),
}

impl From<jsonwebtoken::errors::Error> for Error {
    fn from(e: jsonwebtoken::errors::Error) -> Self {
        Error::JWT(e)
    }
}

/// Provides wrapper around `[u8; JWT_SECRET_LENGTH]` that implements `Zeroize`.
#[derive(Zeroize, Clone)]
#[zeroize(drop)]
pub struct JwtKey([u8; JWT_SECRET_LENGTH]);

impl JwtKey {
    /// Wrap given slice in `Self`. Returns an error if slice.len() != `JWT_SECRET_LENGTH`.
    pub fn from_slice(key: &[u8]) -> Result<Self, String> {
        if key.len() != JWT_SECRET_LENGTH {
            return Err(format!(
                "Invalid key length. Expected {} got {}",
                JWT_SECRET_LENGTH,
                key.len()
            ));
        }
        let mut res = [0; JWT_SECRET_LENGTH];
        res.copy_from_slice(key);
        Ok(Self(res))
    }

    /// Generate a random secret.
    pub fn random() -> Self {
        Self(rand::thread_rng().gen::<[u8; JWT_SECRET_LENGTH]>())
    }

    /// Returns a reference to the underlying byte array.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the hex encoded `String` for the secret.
    pub fn hex_string(&self) -> String {
        hex::encode(self.0)
    }
}

pub fn strip_prefix(s: &str) -> &str {
    if let Some(stripped) = s.strip_prefix("0x") {
        stripped
    } else {
        s
    }
}

/// Contains the JWT secret and claims parameters.
pub struct Auth {
    key: EncodingKey,
    id: Option<String>,
    clv: Option<String>,
}

impl Auth {
    pub fn new(secret: JwtKey, id: Option<String>, clv: Option<String>) -> Self {
        Self { key: EncodingKey::from_secret(secret.as_bytes()), id, clv }
    }

    /// Create a new `Auth` struct given the path to the file containing the hex
    /// encoded jwt key.
    pub fn new_with_path(
        jwt_path: PathBuf,
        id: Option<String>,
        clv: Option<String>,
    ) -> Result<Self, Error> {
        std::fs::read_to_string(&jwt_path)
            .map_err(|e| {
                Error::InvalidKey(format!(
                    "Failed to read JWT secret file {:?}, error: {:?}",
                    jwt_path, e
                ))
            })
            .and_then(|ref s| {
                let secret_bytes = hex::decode(strip_prefix(s.trim_end()))
                    .map_err(|e| Error::InvalidKey(format!("Invalid hex string: {:?}", e)))?;
                let secret = JwtKey::from_slice(&secret_bytes).map_err(Error::InvalidKey)?;
                Ok(Self::new(secret, id, clv))
            })
    }

    /// Generate a JWT token with `claims.iat` set to current time.
    pub fn generate_token(&self) -> Result<String, Error> {
        let claims = self.generate_claims_at_timestamp();
        self.generate_token_with_claims(&claims)
    }

    /// Generate a JWT token with the given claims.
    fn generate_token_with_claims(&self, claims: &Claims) -> Result<String, Error> {
        let header = Header::new(DEFAULT_ALGORITHM);
        Ok(encode(&header, claims, &self.key)?)
    }

    /// Generate a `Claims` struct with `iat` set to current time
    fn generate_claims_at_timestamp(&self) -> Claims {
        Claims { iat: get_current_timestamp(), id: self.id.clone(), clv: self.clv.clone() }
    }

    /// Validate a JWT token given the secret key and return the originally signed `TokenData`.
    pub fn validate_token(
        token: &str,
        secret: &JwtKey,
    ) -> Result<jsonwebtoken::TokenData<Claims>, Error> {
        let mut validation = jsonwebtoken::Validation::new(DEFAULT_ALGORITHM);
        validation.validate_exp = false;
        validation.required_spec_claims.remove("exp");

        jsonwebtoken::decode::<Claims>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
            &validation,
        )
        .map_err(Into::into)
    }
}

/// Claims struct as defined in https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#jwt-claims
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Claims {
    /// issued-at claim. Represented as seconds passed since UNIX_EPOCH.
    iat: u64,
    /// Optional unique identifier for the CL node.
    id: Option<String>,
    /// Optional client version for the CL node.
    clv: Option<String>,
}
