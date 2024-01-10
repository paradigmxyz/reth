use jsonwebtoken::{decode, errors::ErrorKind, Algorithm, DecodingKey, Validation};
use rand::Rng;
use reth_primitives::{
    fs,
    fs::FsPathError,
    hex::{self, encode as hex_encode},
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

/// Errors returned by the [`JwtSecret`]
#[derive(Error, Debug)]
pub enum JwtError {
    /// An error encountered while decoding the hexadecimal string for the JWT secret.
    #[error(transparent)]
    JwtSecretHexDecodeError(#[from] hex::FromHexError),

    /// The JWT key length provided is invalid, expecting a specific length.
    #[error("JWT key is expected to have a length of {0} digits. {1} digits key provided")]
    InvalidLength(usize, usize),

    /// The signature algorithm used in the JWT is not supported. Only HS256 is supported.
    #[error("unsupported signature algorithm. Only HS256 is supported")]
    UnsupportedSignatureAlgorithm,

    /// The provided signature in the JWT is invalid.
    #[error("provided signature is invalid")]
    InvalidSignature,

    /// The "iat" (issued-at) claim in the JWT is not within the allowed ±60 seconds from the
    /// current time.
    #[error("IAT (issued-at) claim is not within ±60 seconds from the current time")]
    InvalidIssuanceTimestamp,

    /// The Authorization header is missing or invalid in the context of JWT validation.
    #[error("Authorization header is missing or invalid")]
    MissingOrInvalidAuthorizationHeader,

    /// An error occurred during JWT decoding.
    #[error("JWT decoding error: {0}")]
    JwtDecodingError(String),

    /// An error related to file system path handling encountered during JWT operations.
    #[error(transparent)]
    JwtFsPathError(#[from] FsPathError),

    /// An I/O error occurred during JWT operations.
    #[error(transparent)]
    IOError(#[from] std::io::Error),
}

/// Length of the hex-encoded 256 bit secret key.
/// A 256-bit encoded string in Rust has a length of 64 digits because each digit represents 4 bits
/// of data. In hexadecimal representation, each digit can have 16 possible values (0-9 and A-F), so
/// 4 bits can be represented using a single hex digit. Therefore, to represent a 256-bit string,
/// we need 64 hexadecimal digits (256 bits ÷ 4 bits per digit = 64 digits).
const JWT_SECRET_LEN: usize = 64;

/// The JWT `iat` (issued-at) claim cannot exceed +-60 seconds from the current time.
const JWT_MAX_IAT_DIFF: Duration = Duration::from_secs(60);

/// The execution layer client MUST support at least the following alg HMAC + SHA256 (HS256)
const JWT_SIGNATURE_ALGO: Algorithm = Algorithm::HS256;

/// Value-object holding a reference to a hex-encoded 256-bit secret key.
/// A JWT secret key is used to secure JWT-based authentication. The secret key is
/// a shared secret between the server and the client and is used to calculate a digital signature
/// for the JWT, which is included in the JWT along with its payload.
///
/// See also: [Secret key - Engine API specs](https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#key-distribution)
#[derive(Clone, PartialEq, Eq)]
pub struct JwtSecret([u8; 32]);

impl JwtSecret {
    /// Creates an instance of [`JwtSecret`].
    ///
    /// Returns an error if one of the following applies:
    /// - `hex` is not a valid hexadecimal string
    /// - `hex` argument length is less than `JWT_SECRET_LEN`
    ///
    /// This strips the leading `0x`, if any.
    pub fn from_hex<S: AsRef<str>>(hex: S) -> Result<Self, JwtError> {
        let hex: &str = hex.as_ref().trim().trim_start_matches("0x");
        if hex.len() != JWT_SECRET_LEN {
            Err(JwtError::InvalidLength(JWT_SECRET_LEN, hex.len()))
        } else {
            let hex_bytes = hex::decode(hex)?;
            // is 32bytes, see length check
            let bytes = hex_bytes.try_into().expect("is expected len");
            Ok(JwtSecret(bytes))
        }
    }

    /// Tries to load a [`JwtSecret`] from the specified file path.
    /// I/O or secret validation errors might occur during read operations in the form of
    /// a [`JwtError`].
    pub fn from_file(fpath: &Path) -> Result<Self, JwtError> {
        let hex = fs::read_to_string(fpath)?;
        let secret = JwtSecret::from_hex(hex)?;
        Ok(secret)
    }

    /// Creates a random [`JwtSecret`] and tries to store it at the specified path. I/O errors might
    /// occur during write operations in the form of a [`JwtError`]
    pub fn try_create(fpath: &Path) -> Result<Self, JwtError> {
        if let Some(dir) = fpath.parent() {
            // Create parent directory
            fs::create_dir_all(dir)?
        }

        let secret = JwtSecret::random();
        let bytes = &secret.0;
        let hex = hex::encode(bytes);
        fs::write(fpath, hex)?;
        Ok(secret)
    }

    /// Validates a JWT token along the following rules:
    /// - The JWT signature is valid.
    /// - The JWT is signed with the `HMAC + SHA256 (HS256)` algorithm.
    /// - The JWT `iat` (issued-at) claim is a timestamp within +-60 seconds from the current time.
    ///
    /// See also: [JWT Claims - Engine API specs](https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#jwt-claims)
    pub fn validate(&self, jwt: String) -> Result<(), JwtError> {
        let mut validation = Validation::new(JWT_SIGNATURE_ALGO);
        // ensure that the JWT has an `iat` claim
        validation.set_required_spec_claims(&["iat"]);
        let bytes = &self.0;

        match decode::<Claims>(&jwt, &DecodingKey::from_secret(bytes), &validation) {
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

    /// Generates a random [`JwtSecret`] containing a hex-encoded 256 bit secret key.
    pub fn random() -> Self {
        let random_bytes: [u8; 32] = rand::thread_rng().gen();
        let secret = hex_encode(random_bytes);
        JwtSecret::from_hex(secret).unwrap()
    }

    /// Encode the header and claims given and sign the payload using the algorithm from the header
    /// and the key.
    ///
    /// ```rust
    /// use reth_rpc::{Claims, JwtSecret};
    ///
    /// let my_claims = Claims { iat: 0, exp: None };
    /// let secret = JwtSecret::random();
    /// let token = secret.encode(&my_claims).unwrap();
    /// ```
    pub fn encode(&self, claims: &Claims) -> Result<String, jsonwebtoken::errors::Error> {
        let bytes = &self.0;
        let key = jsonwebtoken::EncodingKey::from_secret(bytes);
        let algo = jsonwebtoken::Header::new(Algorithm::HS256);
        jsonwebtoken::encode(&algo, claims, &key)
    }
}

impl std::fmt::Debug for JwtSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("JwtSecretHash").field(&"{{}}").finish()
    }
}

impl FromStr for JwtSecret {
    type Err = JwtError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        JwtSecret::from_hex(s)
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
pub struct Claims {
    /// The "iat" value MUST be a number containing a NumericDate value.
    /// According to the RFC A NumericDate represents the number of seconds since
    /// the UNIX_EPOCH.
    /// - [`RFC-7519 - Spec`](https://www.rfc-editor.org/rfc/rfc7519#section-4.1.6)
    /// - [`RFC-7519 - Notations`](https://www.rfc-editor.org/rfc/rfc7519#section-2)
    pub iat: u64,
    /// Expiration, if any
    pub exp: Option<u64>,
}

impl Claims {
    fn is_within_time_window(&self) -> bool {
        let now = SystemTime::now();
        let now_secs = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
        now_secs.abs_diff(self.iat) <= JWT_MAX_IAT_DIFF.as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::jwt_secret::JWT_MAX_IAT_DIFF;
    use assert_matches::assert_matches;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use reth_primitives::fs::FsPathError;
    use std::{
        path::Path,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tempfile::tempdir;

    #[test]
    fn from_hex() {
        let key = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";
        let secret: Result<JwtSecret, _> = JwtSecret::from_hex(key);
        assert!(secret.is_ok());

        let secret: Result<JwtSecret, _> = JwtSecret::from_hex(key);
        assert!(secret.is_ok());
    }

    #[test]
    fn original_key_integrity_across_transformations() {
        let original = "f79ae8046bc11c9927afe911db7143c51a806c4a537cc08e0d37140b0192f430";
        let secret = JwtSecret::from_hex(original).unwrap();
        let bytes = &secret.0;
        let computed = hex_encode(bytes);
        assert_eq!(original, computed);
    }

    #[test]
    fn secret_has_64_hex_digits() {
        let expected_len = 64;
        let secret = JwtSecret::random();
        let hex = hex::encode(secret.0);
        assert_eq!(hex.len(), expected_len);
    }

    #[test]
    fn creation_ok_hex_string_with_0x() {
        let hex: String =
            "0x7365637265747365637265747365637265747365637265747365637265747365".into();
        let result = JwtSecret::from_hex(hex);
        assert!(result.is_ok());
    }

    #[test]
    fn creation_error_wrong_len() {
        let hex = "f79ae8046";
        let result = JwtSecret::from_hex(hex);
        assert!(matches!(result, Err(JwtError::InvalidLength(_, _))));
    }

    #[test]
    fn creation_error_wrong_hex_string() {
        let hex: String = "This__________Is__________Not_______An____Hex_____________String".into();
        let result = JwtSecret::from_hex(hex);
        assert!(matches!(result, Err(JwtError::JwtSecretHexDecodeError(_))));
    }

    #[test]
    fn validation_ok() {
        let secret = JwtSecret::random();
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
        let jwt: String = secret.encode(&claims).unwrap();

        let result = secret.validate(jwt);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn validation_error_iat_out_of_window() {
        let secret = JwtSecret::random();

        // Check past 'iat' claim more than 60 secs
        let offset = Duration::from_secs(JWT_MAX_IAT_DIFF.as_secs() + 1);
        let out_of_window_time = SystemTime::now().checked_sub(offset).unwrap();
        let claims = Claims { iat: to_u64(out_of_window_time), exp: Some(10000000000) };
        let jwt: String = secret.encode(&claims).unwrap();

        let result = secret.validate(jwt);

        assert!(matches!(result, Err(JwtError::InvalidIssuanceTimestamp)));

        // Check future 'iat' claim more than 60 secs
        let offset = Duration::from_secs(JWT_MAX_IAT_DIFF.as_secs() + 1);
        let out_of_window_time = SystemTime::now().checked_add(offset).unwrap();
        let claims = Claims { iat: to_u64(out_of_window_time), exp: Some(10000000000) };
        let jwt: String = secret.encode(&claims).unwrap();

        let result = secret.validate(jwt);

        assert!(matches!(result, Err(JwtError::InvalidIssuanceTimestamp)));
    }

    #[test]
    fn validation_error_wrong_signature() {
        let secret_1 = JwtSecret::random();
        let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
        let jwt: String = secret_1.encode(&claims).unwrap();

        // A different secret will generate a different signature.
        let secret_2 = JwtSecret::random();
        let result = secret_2.validate(jwt);
        assert!(matches!(result, Err(JwtError::InvalidSignature)));
    }

    #[test]
    fn validation_error_unsupported_algorithm() {
        let secret = JwtSecret::random();
        let bytes = &secret.0;

        let key = EncodingKey::from_secret(bytes);
        let unsupported_algo = Header::new(Algorithm::HS384);

        let claims = Claims { iat: to_u64(SystemTime::now()), exp: Some(10000000000) };
        let jwt: String = encode(&unsupported_algo, &claims, &key).unwrap();
        let result = secret.validate(jwt);

        assert!(matches!(result, Err(JwtError::UnsupportedSignatureAlgorithm)));
    }

    #[test]
    fn valid_without_exp_claim() {
        let secret = JwtSecret::random();

        let claims = Claims { iat: to_u64(SystemTime::now()), exp: None };
        let jwt: String = secret.encode(&claims).unwrap();

        let result = secret.validate(jwt);

        assert!(matches!(result, Ok(())));
    }

    #[test]
    fn ephemeral_secret_created() {
        let fpath: &Path = Path::new("secret0.hex");
        assert!(not_exists(fpath));
        JwtSecret::try_create(fpath).expect("A secret file should be created");
        assert!(exists(fpath));
        delete(fpath);
    }

    #[test]
    fn valid_secret_provided() {
        let fpath = Path::new("secret1.hex");
        assert!(not_exists(fpath));

        let secret = JwtSecret::random();
        write(fpath, &hex(&secret));

        match JwtSecret::from_file(fpath) {
            Ok(gen_secret) => {
                delete(fpath);
                assert_eq!(hex(&gen_secret), hex(&secret));
            }
            Err(_) => {
                delete(fpath);
            }
        }
    }

    #[test]
    fn invalid_hex_provided() {
        let fpath = Path::new("secret2.hex");
        write(fpath, "invalid hex");
        let result = JwtSecret::from_file(fpath);
        assert!(result.is_err());
        delete(fpath);
    }

    #[test]
    fn provided_file_not_exists() {
        let fpath = Path::new("secret3.hex");
        let result = JwtSecret::from_file(fpath);
        assert_matches!(result,
            Err(JwtError::JwtFsPathError(FsPathError::Read { source: _, path })) if path == fpath.to_path_buf()
        );
        assert!(!exists(fpath));
    }

    #[test]
    fn provided_file_is_a_directory() {
        let dir = tempdir().unwrap();
        let result = JwtSecret::from_file(dir.path());
        assert_matches!(result, Err(JwtError::JwtFsPathError(FsPathError::Read { source: _, path })) if path == dir.into_path());
    }

    fn hex(secret: &JwtSecret) -> String {
        hex::encode(secret.0)
    }

    fn delete(path: &Path) {
        std::fs::remove_file(path).unwrap();
    }

    fn write(path: &Path, s: &str) {
        std::fs::write(path, s).unwrap();
    }

    fn not_exists(path: &Path) -> bool {
        !exists(path)
    }

    fn exists(path: &Path) -> bool {
        std::fs::metadata(path).is_ok()
    }

    fn to_u64(time: SystemTime) -> u64 {
        time.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }
}
