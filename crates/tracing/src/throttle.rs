//! Throttling utilities for rate-limiting expression execution.

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
        fn should_run(duration: ::core::time::Duration) -> bool {
            use ::core::sync::atomic::Ordering;

            const NOT_YET_RUN: u64 = u64::MAX;

            static START: ::std::sync::LazyLock<::std::time::Instant> =
                ::std::sync::LazyLock::new(::std::time::Instant::now);
            static LAST: ::std::sync::atomic::AtomicU64 =
                ::std::sync::atomic::AtomicU64::new(NOT_YET_RUN);

            let now = START.elapsed().as_millis() as u64;
            let last = LAST.load(Ordering::Relaxed);

            if last == NOT_YET_RUN {
                return LAST
                    .compare_exchange(NOT_YET_RUN, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok();
            }

            let duration_nanos = duration.as_millis() as u64;
            if now.saturating_sub(last) >= duration_nanos {
                LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
            } else {
                false
            }
        }

        if should_run($duration) {
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
