//! Shell-like path expansion utilities.
//!
//! Supports expanding leading `~` to the user's home directory and
//! `$VAR`/`${VAR}` to environment variable values. This is *not* a full
//! shell parser.

use std::path::PathBuf;

/// An error that can occur when expanding a path.
#[derive(Debug, thiserror::Error)]
pub enum ExpandPathError {
    /// Home directory could not be determined.
    #[error("could not determine home directory")]
    NoHomeDir,
    /// Environment variable lookup failed.
    #[error("environment variable `{var_name}` not found: {source}")]
    Var {
        /// The variable name that was looked up.
        var_name: String,
        /// The underlying error.
        source: std::env::VarError,
    },
}

/// Expands a user-specified path, replacing leading `~` with the home directory
/// and `$VAR`/`${VAR}` with environment variable values.
pub fn expand_path(input: &str) -> Result<PathBuf, ExpandPathError> {
    let tilde_expanded = expand_tilde(input)?;
    let expanded = expand_env_vars(&tilde_expanded)?;
    Ok(PathBuf::from(expanded))
}

fn expand_tilde(input: &str) -> Result<String, ExpandPathError> {
    if input == "~" || input.starts_with("~/") || input.starts_with("~\\") {
        let home = dirs_next::home_dir().ok_or(ExpandPathError::NoHomeDir)?;
        let mut out = home.to_string_lossy().into_owned();
        if input.len() > 1 {
            out.push_str(&input[1..]);
        }
        Ok(out)
    } else {
        Ok(input.to_string())
    }
}

fn expand_env_vars(input: &str) -> Result<String, ExpandPathError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '$' {
            result.push(c);
            continue;
        }

        if chars.peek() == Some(&'{') {
            // Braced: ${VAR}
            chars.next();
            let mut name = String::new();
            let mut closed = false;
            while let Some(&c) = chars.peek() {
                if c == '}' {
                    chars.next();
                    closed = true;
                    break;
                }
                name.push(c);
                chars.next();
            }

            if !closed || name.is_empty() {
                // Unterminated `${...` or empty `${}` — keep literal
                result.push_str("${");
                result.push_str(&name);
                if closed {
                    result.push('}');
                }
            } else {
                let value = std::env::var(&name)
                    .map_err(|e| ExpandPathError::Var { var_name: name, source: e })?;
                result.push_str(&value);
            }
        } else {
            // Bare: $VAR
            let mut name = String::new();
            while let Some(&c) = chars.peek() {
                if !c.is_ascii_alphanumeric() && c != '_' {
                    break;
                }
                name.push(c);
                chars.next();
            }

            if name.is_empty() {
                result.push('$');
            } else {
                let value = std::env::var(&name)
                    .map_err(|e| ExpandPathError::Var { var_name: name, source: e })?;
                result.push_str(&value);
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde_home() {
        let home = dirs_next::home_dir().unwrap();
        assert_eq!(expand_path("~").unwrap(), home);
    }

    #[test]
    fn test_expand_tilde_subpath() {
        let home = dirs_next::home_dir().unwrap();
        assert_eq!(expand_path("~/foo/bar").unwrap(), home.join("foo/bar"));
    }

    #[test]
    fn test_tilde_not_at_start() {
        // tilde mid-path should NOT expand
        assert_eq!(expand_path("foo/~/bar").unwrap(), PathBuf::from("foo/~/bar"));
    }

    #[test]
    fn test_expand_env_var_bare() {
        unsafe { std::env::set_var("RETH_TEST_VAR", "hello") };
        assert_eq!(expand_path("$RETH_TEST_VAR/data").unwrap(), PathBuf::from("hello/data"));
        unsafe { std::env::remove_var("RETH_TEST_VAR") };
    }

    #[test]
    fn test_expand_env_var_braced() {
        unsafe { std::env::set_var("RETH_TEST_VAR2", "world") };
        assert_eq!(expand_path("${RETH_TEST_VAR2}/data").unwrap(), PathBuf::from("world/data"));
        unsafe { std::env::remove_var("RETH_TEST_VAR2") };
    }

    #[test]
    fn test_expand_unset_var_errors() {
        let result = expand_path("$RETH_NONEXISTENT_VAR_12345/data");
        assert!(result.is_err());
    }

    #[test]
    fn test_trailing_dollar() {
        // trailing $ should be kept as-is
        assert_eq!(expand_path("foo$").unwrap(), PathBuf::from("foo$"));
    }

    #[test]
    fn test_dollar_non_ident() {
        // $ followed by non-identifier char should be kept as-is
        assert_eq!(expand_path("foo$/bar").unwrap(), PathBuf::from("foo$/bar"));
    }

    #[test]
    fn test_unclosed_brace_is_literal() {
        // unterminated `${...` is kept literally
        assert_eq!(expand_path("${UNCLOSED").unwrap(), PathBuf::from("${UNCLOSED"));
    }

    #[test]
    fn test_empty_braces_literal() {
        // empty `${}` is kept literally
        assert_eq!(expand_path("${}foo").unwrap(), PathBuf::from("${}foo"));
    }

    #[test]
    fn test_no_expansion_needed() {
        assert_eq!(expand_path("/absolute/path").unwrap(), PathBuf::from("/absolute/path"));
        assert_eq!(expand_path("relative/path").unwrap(), PathBuf::from("relative/path"));
    }

    #[test]
    fn test_tilde_and_env_var() {
        unsafe { std::env::set_var("RETH_TEST_DIR", "mydir") };
        let home = dirs_next::home_dir().unwrap();
        assert_eq!(expand_path("~/$RETH_TEST_DIR/data").unwrap(), home.join("mydir/data"));
        unsafe { std::env::remove_var("RETH_TEST_DIR") };
    }
}
