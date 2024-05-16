use crate::providers::StaticFileProvider;

/// Static file provider factory.
pub trait StaticFileProviderFactory {
    /// Create new instance of static file provider.
    fn static_file_provider(&self) -> StaticFileProvider;
}
