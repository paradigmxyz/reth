/// Trait for providers that support configurable static files v2
pub trait StaticFilesConfigurationProvider {
    /// Returns whether static files v2 is enabled
    fn static_files_v2_enabled(&self) -> bool;
}
