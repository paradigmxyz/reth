/// A naive concurrent downloader.
pub mod concurrent;

mod queue;
mod request;

#[cfg(test)]
mod test_utils;
