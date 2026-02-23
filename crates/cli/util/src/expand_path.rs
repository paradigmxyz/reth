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

/// Expands a leading `~` or `~/` (or `~\` on Windows) to the user's home
/// directory. A `~` appearing anywhere else in the path is left unchanged.
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

/// Replaces `$VAR` and `${VAR}` references in `input` with their environment
/// variable values.
///
/// # Syntax
///
/// | Pattern  | Behavior                                              |
/// |----------|-------------------------------------------------------|
/// | `$VAR`   | Expands a bare variable (name = `[A-Za-z0-9_]+`)     |
/// | `${VAR}` | Expands a braced variable                             |
/// | `$`      | A trailing or isolated `$` is kept literally          |
/// | `${}`    | Empty braces are kept literally                       |
/// | `${X`    | Unterminated braces are kept literally                |
///
/// # Errors
///
/// Returns [`ExpandPathError::Var`] if a well-formed variable name references
/// an unset environment variable.
fn expand_env_vars(input: &str) -> Result<String, ExpandPathError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '$' {
            result.push(c);
            continue;
        }

        match chars.peek() {
            Some(&'{') => expand_braced_var(&mut chars, &mut result)?,
            Some(&c) if c.is_ascii_alphanumeric() || c == '_' => {
                expand_bare_var(&mut chars, &mut result)?;
            }
            _ => result.push('$'), // trailing or isolated `$`
        }
    }

    Ok(result)
}

/// Parses and expands `${VAR}`. Assumes the leading `$` has already been
/// consumed and `chars` is pointing at `{`.
fn expand_braced_var(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    result: &mut String,
) -> Result<(), ExpandPathError> {
    chars.next(); // consume '{'

    let mut name = String::new();
    let mut closed = false;
    for c in chars.by_ref() {
        if c == '}' {
            closed = true;
            break;
        }
        name.push(c);
    }

    if !closed || name.is_empty() {
        // Malformed — keep as literal text
        result.push_str("${");
        result.push_str(&name);
        if closed {
            result.push('}');
        }
        return Ok(());
    }

    result.push_str(&lookup_var(&name)?);
    Ok(())
}

/// Parses and expands `$VAR`. Assumes the leading `$` has already been consumed
/// and `chars` is pointing at the first character of the variable name.
fn expand_bare_var(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    result: &mut String,
) -> Result<(), ExpandPathError> {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if !c.is_ascii_alphanumeric() && c != '_' {
            break;
        }
        name.push(c);
        chars.next();
    }

    result.push_str(&lookup_var(&name)?);
    Ok(())
}

/// Looks up an environment variable by name, returning an [`ExpandPathError`]
/// on failure.
fn lookup_var(name: &str) -> Result<String, ExpandPathError> {
    std::env::var(name).map_err(|e| ExpandPathError::Var { var_name: name.to_owned(), source: e })
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

    #[test]
    fn test_multiple_env_vars() {
        unsafe { std::env::set_var("RETH_TEST_CHAIN", "mainnet") };
        unsafe { std::env::set_var("RETH_TEST_NET", "prod") };
        assert_eq!(
            expand_path("/data/$RETH_TEST_CHAIN/$RETH_TEST_NET/db").unwrap(),
            PathBuf::from("/data/mainnet/prod/db")
        );
        unsafe { std::env::remove_var("RETH_TEST_CHAIN") };
        unsafe { std::env::remove_var("RETH_TEST_NET") };
    }

    #[test]
    fn test_multiple_braced_env_vars() {
        unsafe { std::env::set_var("RETH_TEST_A", "alpha") };
        unsafe { std::env::set_var("RETH_TEST_B", "beta") };
        assert_eq!(
            expand_path("${RETH_TEST_A}/${RETH_TEST_B}").unwrap(),
            PathBuf::from("alpha/beta")
        );
        unsafe { std::env::remove_var("RETH_TEST_A") };
        unsafe { std::env::remove_var("RETH_TEST_B") };
    }

    #[test]
    fn test_repeated_same_var() {
        unsafe { std::env::set_var("RETH_TEST_REP", "val") };
        assert_eq!(
            expand_path("$RETH_TEST_REP/$RETH_TEST_REP/db").unwrap(),
            PathBuf::from("val/val/db")
        );
        unsafe { std::env::remove_var("RETH_TEST_REP") };
    }

    #[test]
    fn test_env_var_with_suffix() {
        // bare $VAR stops at non-ident char, so "-db" is literal suffix
        unsafe { std::env::set_var("RETH_TEST_CHAIN2", "mainnet") };
        assert_eq!(expand_path("$RETH_TEST_CHAIN2-db").unwrap(), PathBuf::from("mainnet-db"));
        unsafe { std::env::remove_var("RETH_TEST_CHAIN2") };
    }

    #[test]
    fn test_braced_env_var_with_suffix() {
        // braced ${VAR} allows immediate suffix
        unsafe { std::env::set_var("RETH_TEST_CHAIN3", "mainnet") };
        assert_eq!(expand_path("${RETH_TEST_CHAIN3}db").unwrap(), PathBuf::from("mainnetdb"));
        unsafe { std::env::remove_var("RETH_TEST_CHAIN3") };
    }

    #[test]
    fn test_env_var_preceded_by_prefix() {
        unsafe { std::env::set_var("RETH_TEST_CHAIN4", "mainnet") };
        assert_eq!(
            expand_path("prefix_$RETH_TEST_CHAIN4/suffix").unwrap(),
            PathBuf::from("prefix_mainnet/suffix")
        );
        unsafe { std::env::remove_var("RETH_TEST_CHAIN4") };
    }

    #[test]
    fn test_env_var_empty_value() {
        // env var set to empty string should expand to empty, not error
        unsafe { std::env::set_var("RETH_TEST_EMPTY", "") };
        assert_eq!(expand_path("/data/$RETH_TEST_EMPTY/db").unwrap(), PathBuf::from("/data//db"));
        unsafe { std::env::remove_var("RETH_TEST_EMPTY") };
    }

    #[test]
    fn test_tilde_user_does_not_expand() {
        // ~username should NOT expand (we only support bare ~)
        assert_eq!(expand_path("~user/foo").unwrap(), PathBuf::from("~user/foo"));
    }

    #[test]
    fn test_tilde_trailing_slash_only() {
        let home = dirs_next::home_dir().unwrap();
        let expected = PathBuf::from(format!("{}/", home.display()));
        assert_eq!(expand_path("~/").unwrap(), expected);
    }

    #[test]
    fn test_tilde_backslash() {
        // Windows-style ~\ should also trigger tilde expansion
        let home = dirs_next::home_dir().unwrap();
        let result = expand_path("~\\foo\\bar").unwrap();
        let result_str = result.to_string_lossy();
        let home_str = home.to_string_lossy();
        assert!(
            result_str.starts_with(home_str.as_ref()),
            "expected result '{result_str}' to start with home '{home_str}'"
        );
        assert!(result_str.ends_with("\\foo\\bar"));
    }

    #[test]
    fn test_env_var_with_underscores_and_digits() {
        unsafe { std::env::set_var("RETH_V2_TEST_99", "ok") };
        assert_eq!(expand_path("$RETH_V2_TEST_99").unwrap(), PathBuf::from("ok"));
        unsafe { std::env::remove_var("RETH_V2_TEST_99") };
    }

    #[test]
    fn test_realistic_datadir_path() {
        // mimics real --datadir usage: ~/.reth/mainnet
        let home = dirs_next::home_dir().unwrap();
        assert_eq!(expand_path("~/.reth/mainnet").unwrap(), home.join(".reth/mainnet"));
    }

    #[test]
    fn test_realistic_env_datadir_path() {
        // mimics $HOME/.reth pattern
        let home = dirs_next::home_dir().unwrap();
        let home_str = home.to_string_lossy();
        unsafe { std::env::set_var("RETH_TEST_HOME", home_str.as_ref()) };
        assert_eq!(
            expand_path("$RETH_TEST_HOME/.reth/mainnet").unwrap(),
            home.join(".reth/mainnet")
        );
        unsafe { std::env::remove_var("RETH_TEST_HOME") };
    }
}
