use super::*;
use std::{
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
    task::Waker,
    thread,
};

#[test]
fn routes_borrowed_jobs_and_restores_context() {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let caller = thread::current().id();
    let mut ids = Vec::new();
    in_place_scope(&pool, |scope| scope.spawn(|| ids.push(thread::current().id())));
    assert_ne!(ids[0], caller);
    inline(|| {
        assert!(is_inline());
        inline(|| assert!(is_inline()));
        assert!(is_inline());
        assert_eq!(join(|| thread::current().id(), || thread::current().id()), (caller, caller));
        in_place_scope(&pool, |scope| scope.spawn(|| ids.push(thread::current().id())));
        assert_eq!(map_collect(0..3, |_| thread::current().id()), vec![caller; 3]);
        spawn(move || assert_eq!(thread::current().id(), caller));
    });
    assert_eq!(ids[1], caller);
    assert!(!is_inline());
}

#[test]
fn children_finish_before_panic_and_context_is_restored() {
    let second = AtomicBool::new(false);
    assert!(catch_unwind(|| inline(|| join(
        || panic!("first"),
        || second.store(true, AtomicOrdering::Relaxed)
    )))
    .is_err());
    assert!(second.load(AtomicOrdering::Relaxed));
    assert!(!is_inline());
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    second.store(false, AtomicOrdering::Relaxed);
    assert!(catch_unwind(AssertUnwindSafe(|| inline(|| in_place_scope(&pool, |scope| {
        scope.spawn(|| panic!("child"));
        scope.spawn(|| second.store(true, AtomicOrdering::Relaxed));
    }))))
    .is_err());
    assert!(second.load(AtomicOrdering::Relaxed));
    assert!(!is_inline());
}

#[test]
fn native_and_inline_results_match() {
    fn compute() -> (Vec<(u32, u32)>, Vec<u32>, Vec<u32>) {
        let mut stable = vec![(2, 0), (1, 1), (2, 2), (1, 3)];
        sort_by_key(&mut stable, |item| item.0);
        let mut unstable = map_collect(0..1000, |i| 999 - i);
        sort_unstable_by_key(&mut unstable, |i| *i);
        let mut reversed = unstable.clone();
        sort_unstable_by(&mut reversed, |a, b| b.cmp(a));
        assert_eq!(try_map_collect(0..4, |i| if i == 2 { Err(i) } else { Ok(i) }), Err(2));
        (stable, unstable, reversed)
    }
    let native = compute();
    assert_eq!(native.0, vec![(1, 1), (1, 3), (2, 0), (2, 2)]);
    assert_eq!(native, inline(compute));
}

#[test]
fn future_context_survives_migration_and_cancellation() {
    struct Pending;
    impl Future for Pending {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            assert!(is_inline());
            Poll::Pending
        }
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            assert!(is_inline());
        }
    }
    let mut future = Box::pin(deterministic(Pending));
    assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    assert!(!is_inline());
    thread::spawn(move || {
        assert!(!is_inline());
        assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        assert!(!is_inline());
        drop(future);
        assert!(!is_inline());
    })
    .join()
    .unwrap();
    assert!(!is_inline());
}

#[test]
fn explicit_pool_routes_do_not_submit_inline() {
    inline(|| {
        spawn_with(|| assert!(is_inline()), |_| panic!("native submission"));
        assert_eq!(run_with(|| 42, |_| panic!("native submission")), 42);
        assert_eq!(filter_map_collect(0..6, |i| (i % 2 == 0).then_some(i)), vec![0, 2, 4]);
    });
    let submitted = AtomicBool::new(false);
    spawn_with(
        || (),
        |job| {
            submitted.store(true, AtomicOrdering::Relaxed);
            job();
        },
    );
    assert!(submitted.load(AtomicOrdering::Relaxed));
}

#[test]
fn opaque_parallel_iterator_uses_one_synchronous_worker() {
    let caller = thread::current().id();
    let run = || {
        inline(|| {
            collect((0..128).into_par_iter().filter(|i| i % 2 == 0).map(|i| {
                assert!(is_inline());
                // Nested opaque collection must reuse the worker, without a recursive pool
                // submission.
                assert_eq!(collect((0..2).into_par_iter()), vec![0, 1]);
                (i, thread::current().id())
            }))
        })
    };
    let result = run();
    assert_ne!(caller, result[0].1);
    assert!(result.iter().all(|(_, thread)| *thread == result[0].1));
    assert_eq!(
        result.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        (0..128).step_by(2).collect::<Vec<_>>()
    );
    assert_eq!(result, run());
    assert!(catch_unwind(|| inline(|| collect((0..8).into_par_iter().map(|_| panic!("item")))))
        .is_err());
    assert!(!is_inline());
    assert_eq!(result, run());
}
