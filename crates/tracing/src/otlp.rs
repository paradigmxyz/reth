/// OTLP protocol options.
#[derive(Debug, Clone, Copy)]
pub enum OtlpProtocols {
    /// GRPC.
    Grpc,
    /// Binary.
    Binary,
    /// JSON.
    Json,
}

impl std::str::FromStr for OtlpProtocols {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            s if s.eq_ignore_ascii_case("grpc") => Ok(Self::Grpc),
            s if s.eq_ignore_ascii_case("binary") => Ok(Self::Binary),
            s if s.eq_ignore_ascii_case("json") => Ok(Self::Json),
            _ => Err(format!("Invalid protocol: {}", s)),
        }
    }
}

impl From<OtlpProtocols> for opentelemetry_otlp::Protocol {
    fn from(protocol: OtlpProtocols) -> Self {
        match protocol {
            OtlpProtocols::Grpc => Self::Grpc,
            OtlpProtocols::Binary => Self::HttpBinary,
            OtlpProtocols::Json => Self::HttpJson,
        }
    }
}

/// OTLP configuration.
#[derive(Debug, Clone)]
pub struct OtlpConfig {
    /// Endpoint url to which to send spans and events. The endpoint URL must
    /// be a valid URL, including the protocol prefix (http or https) and any
    /// http basic auth information.
    pub url: String,
    /// The protocol to use for sending spans and events.
    pub protocol: OtlpProtocols,
    /// The [`EnvFilter`] directive to use for spans and events exported to the
    /// `OpenTelemetry` layer.
    ///
    /// [`EnvFilter`]: tracing_subscriber::filter::EnvFilter
    pub directive: String,
    /// The timeout for sending spans and events.
    pub timeout: u64,
}
