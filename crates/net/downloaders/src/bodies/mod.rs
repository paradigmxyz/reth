/// A naive concurrent downloader.
pub mod bodies;

/// TODO:
pub mod task;

mod queue;
mod request;

#[cfg(test)]
mod test_utils;
