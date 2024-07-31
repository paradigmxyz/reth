
/// Helper trait used to validate responses.
pub trait EthResponseValidator {
    /// Determine whether the response matches what we requested in [`HeadersRequest`]
    fn is_likely_bad_headers_response(&self, request: &HeadersRequest) -> bool;

    /// Return the response reputation impact if any
    fn reputation_change_err(&self) -> Option<ReputationChangeKind>;
}

impl EthResponseValidator for RequestResult<Vec<Header>> {
    fn is_likely_bad_headers_response(&self, request: &HeadersRequest) -> bool {
        match self {
            Ok(headers) => {
                let request_length = headers.len() as u64;

                if request_length <= 1 && request.limit != request_length {
                    return true
                }

                match request.start {
                    BlockHashOrNumber::Number(block_number) => headers
                        .first()
                        .map(|header| block_number != header.number)
                        .unwrap_or_default(),
                    BlockHashOrNumber::Hash(_) => {
                        // we don't want to hash the header
                        false
                    }
                }
            }
            Err(_) => true,
        }
    }

    /// [`RequestError::ChannelClosed`] is not possible here since these errors are mapped to
    /// `ConnectionDropped`, which will be handled when the dropped connection is cleaned up.
    ///
    /// [`RequestError::ConnectionDropped`] should be ignored here because this is already handled
    /// when the dropped connection is handled.
    ///
    /// [`RequestError::UnsupportedCapability`] is not used yet because we only support active
    /// session for eth protocol.
    fn reputation_change_err(&self) -> Option<ReputationChangeKind> {
        if let Err(err) = self {
            match err {
                RequestError::ChannelClosed |
                RequestError::ConnectionDropped |
                RequestError::UnsupportedCapability |
                RequestError::BadResponse => None,
                RequestError::Timeout => Some(ReputationChangeKind::Timeout),
            }
        } else {
            None
        }
    }
}
