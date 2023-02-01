use std::time::{SystemTime, UNIX_EPOCH};

use hex::encode as hex_encode;
use jsonwebtoken::{decode, errors::ErrorKind, Algorithm, DecodingKey, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by the [`JwtSecret`][crate::engine::JwtSecret]
/// and the [`Jwt`][crate::engine::JwtValidator] structs.
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum JwtError {
    #[error(transparent)]
    JwtSecretHexDecodeError(#[from] hex::FromHexError),
    #[error("JWT key is expected to have a length of {0} digits. {1} digits key provided.")]
    InvalidLength(usize, usize),
    #[error("Unsupported signature algorithm. Only HS256 is supported")]
    UnsupportedSignatureAlgorithm,
    #[error("The provided signature is invalid.")]
    InvalidSignature,
    #[error("The iat (issued-at) claim is not within +-60 seconds from the current time")]
    InvalidIssuanceTimestamp,
    #[error("JWT decoding error {0}")]
    JwtDecodingError(String),
}

/// Length of the hex-encoded 256 bit secret key.
/// A 256-bit encoded string in Rust has a length of 64 digits because each digit represents 4 bits
/// of data. In hexadecimal representation, each digit can have 16 possible values (0-9 and A-F), so
/// 4 bits can be represented using a single hex digit. Therefore, to represent a 256-bit string,
/// we need 64 hexadecimal digits (256 bits ÷ 4 bits per digit = 64 digits).
const JWT_SECRET_LEN: usize = 64;

/// The JWT `iat` (issued-at) claim cannot exceed +-60 seconds from the current time.
const JWT_MAX_IAT_DIFF: u64 = 60;

/// The execution layer client MUST support at least the following alg HMAC + SHA256 (HS256)
const JWT_SIGNATURE_ALGO: Algorithm = Algorithm::HS256;

/// Value-object holding a reference to an hex-encoded 256 bit secret key.
/// A JWT secret key is used to secure JWT-based authentication. The secret key is
/// a shared secret between the server and the clienta and is used to calculate a digital signature
/// for the JWT, which is included in the JWT along with its payload.
/// -----------------------------------------------------------------
/// [Secret key - Engine API specs](https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#key-distribution)
#[derive(Debug)]
pub struct JwtSecret(String);

impl JwtSecret {
    /// Creates an instance of [`JwtSecret`][crate::engine::JwtSecret].
    /// Generates and error if one of the following applies:
    /// - The `hex` argument is not a valid hexadecimal string
    /// - The `hex` argument length is less than `JWT_SECRET_LEN`
    pub fn new(hex: String) -> Result<JwtSecret, JwtError> {
        if hex.len() != JWT_SECRET_LEN {
            Err(JwtError::InvalidLength(JWT_SECRET_LEN, hex.len()))
        } else {
            hex::decode(&hex)?;
            Ok(Self(hex))
        }
    }

    /// Generates a random [`JwtSecret`][crate::engine::JwtSecret]
    /// containing a hex-encoded 256 bit secret key.
    pub fn random() -> Self {
        let random_bytes: [u8; 32] = rand::thread_rng().gen();
        let secret = hex_encode(random_bytes);
        Self(secret)
    }

    /// Returns a string representation of the secret
    pub fn as_string(&self) -> String {
        self.0.clone()
    }
}

/// Struct for validating JWT tokens.
#[derive(Debug)]
pub struct JwtValidator(JwtSecret);

impl From<JwtSecret> for JwtValidator {
    fn from(secret: JwtSecret) -> Self {
        Self(secret)
    }
}

impl JwtValidator {
    /// Validates a JWT token along the following rules:
    /// - The JWT signature is valid.
    /// - The JWT is signed with the `HMAC + SHA256 (HS256)` algorithm.
    /// - The JWT `iat` (issued-at) claim is a timestamp within +-60 seconds from the current time.
    /// -----------------------------------------------------------------
    /// [JWT Claims - Engine API specs](https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#jwt-claims)
    pub fn validate(&self, jwt: String) -> Result<(), JwtError> {
        let validation = Validation::new(JWT_SIGNATURE_ALGO);
        let key = self.0.as_string();

        match decode::<Claims>(&jwt, &DecodingKey::from_secret(key.as_bytes()), &validation) {
            Ok(token) => {
                if !token.claims.is_within_time_window() {
                    Err(JwtError::InvalidIssuanceTimestamp)?
                }
            }
            Err(err) => match *err.kind() {
                ErrorKind::InvalidSignature => Err(JwtError::InvalidSignature)?,
                ErrorKind::InvalidAlgorithm => Err(JwtError::UnsupportedSignatureAlgorithm)?,
                _ => {
                    let detail = format!("{err:?}");
                    Err(JwtError::JwtDecodingError(detail))?
                }
            },
        };

        Ok(())
    }
}

/// Claims in JWT are used to represent a set of information about an entity.
/// Claims are essentially key-value pairs that are encoded as JSON objects and included in the
/// payload of a JWT. They are used to transmit information such as the identity of the entity, the
/// time the JWT was issued, and the expiration time of the JWT, among others.
///
/// The Engine API spec requires that just the `iat` (issued-at) claim is provided.
/// It ignores claims that are optional or additional for this specification.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    iat: std::time::SystemTime,
    exp: u64,
}
impl Claims {
    fn is_within_time_window(&self) -> bool {
        let now = SystemTime::now();
        let now_secs: u64 = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let iat_secs: u64 = self.iat.duration_since(UNIX_EPOCH).unwrap().as_secs();
        now_secs.abs_diff(iat_secs) <= JWT_MAX_IAT_DIFF
    }
}

#[cfg(test)]
mod tests_jwtsecret {
    use crate::engine::JwtSecret;

    use super::JwtError;

    #[test]
    fn secret_has_64_hex_digits() {
        let expected_len = 64;
        let jwt = JwtSecret::random();
        assert_eq!(jwt.0.len(), expected_len);
    }

    #[test]
    fn creation_error_wrong_len() {
        let hex: String = "f79ae8046".into();
        let result = JwtSecret::new(hex);
        assert!(matches!(result, Err(JwtError::InvalidLength(_, _))));
    }

    #[test]
    fn creation_error_wrong_hex_string() {
        let hex: String = "This__________Is__________Not_______An____Hex_____________String".into();
        let result = JwtSecret::new(hex);
        assert!(matches!(result, Err(JwtError::JwtSecretHexDecodeError(_))));
    }
}

#[cfg(test)]
mod tests_jwt_validation {
    use crate::engine::jwt_secret::{JWT_MAX_IAT_DIFF, JWT_SIGNATURE_ALGO};

    use super::{Claims, JwtError, JwtSecret, JwtValidator};
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use std::time::{Duration, SystemTime};

    #[test]
    fn validation_ok() {
        let secret = JwtSecret::random();
        let key_str = secret.as_string();
        let validator = JwtValidator::from(secret);

        let key = EncodingKey::from_secret(key_str.as_bytes());
        let algo = Header::new(JWT_SIGNATURE_ALGO);
        let claims = Claims { iat: SystemTime::now(), exp: 10000000000 };

        let jwt: String = encode(&algo, &claims, &key).unwrap();
        let result = validator.validate(jwt);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn validation_error_iat_out_of_window() {
        let secret = JwtSecret::random();
        let key_str = secret.as_string();
        let validator = JwtValidator::from(secret);

        let key = EncodingKey::from_secret(key_str.as_bytes());
        let algo = Header::new(JWT_SIGNATURE_ALGO);

        // Check past 'iat' claim more than 60 secs
        let offset = Duration::from_secs(JWT_MAX_IAT_DIFF + 1);
        let out_of_window_time = SystemTime::now().checked_sub(offset).unwrap();
        let claims = Claims { iat: out_of_window_time, exp: 10000000000 };
        let jwt: String = encode(&algo, &claims, &key).unwrap();
        let result = validator.validate(jwt);

        assert!(matches!(result, Err(JwtError::InvalidIssuanceTimestamp)));

        // Check future 'iat' claim more than 60 secs
        let offset = Duration::from_secs(JWT_MAX_IAT_DIFF + 1);
        let out_of_window_time = SystemTime::now().checked_add(offset).unwrap();
        let claims = Claims { iat: out_of_window_time, exp: 10000000000 };
        let jwt: String = encode(&algo, &claims, &key).unwrap();
        let result = validator.validate(jwt);

        assert!(matches!(result, Err(JwtError::InvalidIssuanceTimestamp)));
    }

    #[test]
    fn validation_error_wrong_signature() {
        let claims = Claims { iat: SystemTime::now(), exp: 10000000000 };
        let wrong_secret = b"this_is_a_wrong_secret";
        let key = EncodingKey::from_secret(wrong_secret);
        let algo = Header::new(JWT_SIGNATURE_ALGO);
        let jwt: String = encode(&algo, &claims, &key).unwrap();

        // Configure a JwtValidator with a different secret.
        // This will generate a different signature.
        let secret = JwtSecret::random();
        let validator = JwtValidator::from(secret);

        let result = validator.validate(jwt);
        assert!(matches!(result, Err(JwtError::InvalidSignature)));
    }

    #[test]
    fn validation_error_unsupported_algorithm() {
        let secret = JwtSecret::random();
        let key_str = secret.as_string();
        let validator = JwtValidator::from(secret);

        let key = EncodingKey::from_secret(key_str.as_bytes());
        let unsupported_algo = Header::new(Algorithm::HS384);

        let claims = Claims { iat: SystemTime::now(), exp: 10000000000 };
        let jwt: String = encode(&unsupported_algo, &claims, &key).unwrap();
        let result = validator.validate(jwt);

        assert!(matches!(result, Err(JwtError::UnsupportedSignatureAlgorithm)));
    }
}
