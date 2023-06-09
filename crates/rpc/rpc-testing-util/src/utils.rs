//! Utils for testing RPC.

/// This will read the value of the given environment variable and parse it as a URL.
///
/// If the value has no http(s) scheme, it will be appended: `http://{var}`.
pub fn parse_env_url(var: &str) -> Result<String, std::env::VarError> {
    let var = std::env::var(var)?;
    if var.starts_with("http") {
        Ok(var)
    } else {
        Ok(format!("http://{}", var))
    }
}
