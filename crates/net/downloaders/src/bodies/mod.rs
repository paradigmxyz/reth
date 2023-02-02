/// A naive concurrent downloader.
#[allow(clippy::module_inception)]
pub mod bodies;

/// TODO:
pub mod task;

mod queue;
mod request;

#[cfg(test)]
pub mod test_utils;
