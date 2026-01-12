//! Throttling utilities for rate-limiting expression execution.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    },
    time::Instant,
};

/// Sentinel value indicating the throttle has never run.
#[doc(hidden)]
pub const NOT_YET_RUN: u64 = u64::MAX;

/// Checks if enough time has passed since the last run. Implementation detail for [`throttle!`].
#[doc(hidden)]
pub fn should_run(start: &LazyLock<Instant>, last: &AtomicU64, duration_millis: u64) -> bool {
    let now = start.elapsed().as_millis() as u64;
    let last_val = last.load(Ordering::Relaxed);

    if last_val == NOT_YET_RUN {
        return last
            .compare_exchange(NOT_YET_RUN, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok();
    }

    if now.saturating_sub(last_val) >= duration_millis {
        last.compare_exchange(last_val, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    } else {
        false
    }
}

/// Throttles the execution of an expression to run at most once per specified duration.
///
/// Uses static variables with lazy initialization to track the last execution time.
/// Thread-safe via atomic operations.
///
/// # Examples
///
/// ```ignore
/// use std::time::Duration;
/// use reth_tracing::throttle;
///
/// // Log at most once per second.
/// throttle!(Duration::from_secs(1), || {
///     tracing::info!("This message is throttled");
/// });
/// ```
#[macro_export]
macro_rules! throttle {
    ($duration:expr, || $expr:expr) => {{
        static START: ::std::sync::LazyLock<::std::time::Instant> =
            ::std::sync::LazyLock::new(::std::time::Instant::now);
        static LAST: ::core::sync::atomic::AtomicU64 =
            ::core::sync::atomic::AtomicU64::new($crate::__private::NOT_YET_RUN);

        if $crate::__private::should_run(&START, &LAST, $duration.as_millis() as u64) {
            $expr
        }
    }};
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_throttle_runs_once_initially() {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        throttle!(std::time::Duration::from_secs(10), || {
            COUNTER.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    }
}
