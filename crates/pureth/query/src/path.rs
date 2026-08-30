#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathToken {
    Field(String),
    Index(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    EmptyPath,
    UnexpectedByte { offset: usize, byte: u8 },
    MissingField { offset: usize },
    MissingIndex { offset: usize },
    LeadingZero { offset: usize },
    MissingClosingBracket { offset: usize },
    IndexOverflow { offset: usize },
}

pub fn parse_path(path: &str) -> Result<Vec<PathToken>, ParseError> {
    if path.is_empty() {
        return Err(ParseError::EmptyPath);
    }

    let bytes = path.as_bytes();
    let mut cursor = 0;
    let mut tokens = Vec::new();

    while cursor < bytes.len() {
        match bytes[cursor] {
            b'.' => {
                let field_start = cursor + 1;
                if field_start >= bytes.len() || !bytes[field_start].is_ascii_alphabetic() {
                    return Err(ParseError::MissingField { offset: field_start });
                }

                cursor = field_start + 1;
                while cursor < bytes.len() &&
                    (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
                {
                    cursor += 1;
                }

                tokens.push(PathToken::Field(path[field_start..cursor].to_owned()));
            }
            b'[' => {
                let index_start = cursor + 1;
                if index_start >= bytes.len() || bytes[index_start] == b']' {
                    return Err(ParseError::MissingIndex { offset: index_start });
                }
                if !bytes[index_start].is_ascii_digit() {
                    return Err(ParseError::UnexpectedByte {
                        offset: index_start,
                        byte: bytes[index_start],
                    });
                }
                if bytes[index_start] == b'0' &&
                    bytes.get(index_start + 1).is_some_and(u8::is_ascii_digit)
                {
                    return Err(ParseError::LeadingZero { offset: index_start });
                }

                cursor = index_start;
                let mut index = 0_u64;
                while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                    index = index
                        .checked_mul(10)
                        .and_then(|value| value.checked_add(u64::from(bytes[cursor] - b'0')))
                        .ok_or(ParseError::IndexOverflow { offset: index_start })?;
                    cursor += 1;
                }

                if cursor >= bytes.len() {
                    return Err(ParseError::MissingClosingBracket { offset: cursor });
                }
                if bytes[cursor] != b']' {
                    return Err(ParseError::UnexpectedByte { offset: cursor, byte: bytes[cursor] });
                }

                tokens.push(PathToken::Index(index));
                cursor += 1;
            }
            byte => {
                return Err(ParseError::UnexpectedByte { offset: cursor, byte });
            }
        }
    }

    Ok(tokens)
}
