//! Test demonstrating that moving blocking recv() outside the write lock
//! eliminates reader contention on the execution cache.

#[cfg(test)]
mod tests {
    use parking_lot::RwLock;
    use std::{
        sync::{mpsc, Arc},
        thread,
        time::{Duration, Instant},
    };

    /// Simulates the OLD behavior: blocking recv() inside the write lock.
    /// Readers are blocked for the entire recv() duration.
    #[test]
    fn old_behavior_blocks_readers() {
        let lock = Arc::new(RwLock::new(0u64));
        let (tx, rx) = mpsc::channel::<()>();

        let lock_clone = lock.clone();

        let writer = thread::spawn(move || {
            let mut guard = lock_clone.write();
            let _ = rx.recv();
            *guard = 42;
        });

        thread::sleep(Duration::from_millis(10));

        let lock_clone2 = lock.clone();
        let reader = thread::spawn(move || {
            let start = Instant::now();
            let _val = lock_clone2.read();
            start.elapsed()
        });

        thread::sleep(Duration::from_millis(100));
        tx.send(()).unwrap();

        writer.join().unwrap();
        let reader_blocked_for = reader.join().unwrap();

        assert!(
            reader_blocked_for >= Duration::from_millis(80),
            "OLD behavior: reader should be blocked while recv() waits inside write lock, \
             but was only blocked for {:?}",
            reader_blocked_for
        );
        println!("OLD behavior: reader blocked for {:?} (expected ~100ms)", reader_blocked_for);
    }

    /// Simulates the NEW behavior: recv() outside the write lock.
    /// Readers are NOT blocked during recv() wait.
    #[test]
    fn new_behavior_does_not_block_readers() {
        let lock = Arc::new(RwLock::new(0u64));
        let (tx, rx) = mpsc::channel::<()>();

        let lock_clone = lock.clone();

        let writer = thread::spawn(move || {
            let new_value = 42u64;
            let _ = rx.recv();

            let mut guard = lock_clone.write();
            *guard = new_value;
            drop(guard);
        });

        thread::sleep(Duration::from_millis(10));

        let lock_clone2 = lock.clone();
        let reader = thread::spawn(move || {
            let start = Instant::now();
            let _val = lock_clone2.read();
            start.elapsed()
        });

        let reader_blocked_for = reader.join().unwrap();

        thread::sleep(Duration::from_millis(100));
        tx.send(()).unwrap();
        writer.join().unwrap();

        assert!(
            reader_blocked_for < Duration::from_millis(5),
            "NEW behavior: reader should NOT be blocked during recv() wait, \
             but was blocked for {:?}",
            reader_blocked_for
        );
        println!("NEW behavior: reader blocked for {:?} (expected <1ms)", reader_blocked_for);
    }

    /// Side-by-side comparison showing the lock contention difference
    #[test]
    fn lock_contention_comparison() {
        let validation_delay = Duration::from_millis(100);
        let num_readers = 4;

        // --- OLD BEHAVIOR ---
        let old_reader_waits = {
            let lock = Arc::new(RwLock::new(0u64));
            let (tx, rx) = mpsc::channel::<()>();
            let lock_w = lock.clone();

            let writer = thread::spawn(move || {
                let mut guard = lock_w.write();
                let _ = rx.recv();
                *guard = 1;
            });

            thread::sleep(Duration::from_millis(5));

            let readers: Vec<_> = (0..num_readers)
                .map(|_| {
                    let l = lock.clone();
                    thread::spawn(move || {
                        let start = Instant::now();
                        let _v = l.read();
                        start.elapsed()
                    })
                })
                .collect();

            thread::sleep(validation_delay);
            tx.send(()).unwrap();
            writer.join().unwrap();
            readers.into_iter().map(|r| r.join().unwrap()).collect::<Vec<_>>()
        };

        // --- NEW BEHAVIOR ---
        let new_reader_waits = {
            let lock = Arc::new(RwLock::new(0u64));
            let (tx, rx) = mpsc::channel::<()>();
            let lock_w = lock.clone();

            let writer = thread::spawn(move || {
                let _ = rx.recv();
                let mut guard = lock_w.write();
                *guard = 1;
                drop(guard);
            });

            thread::sleep(Duration::from_millis(5));

            let readers: Vec<_> = (0..num_readers)
                .map(|_| {
                    let l = lock.clone();
                    thread::spawn(move || {
                        let start = Instant::now();
                        let _v = l.read();
                        start.elapsed()
                    })
                })
                .collect();

            let waits: Vec<_> = readers.into_iter().map(|r| r.join().unwrap()).collect();
            thread::sleep(validation_delay);
            tx.send(()).unwrap();
            writer.join().unwrap();
            waits
        };

        let old_max = old_reader_waits.iter().max().unwrap();
        let new_max = new_reader_waits.iter().max().unwrap();

        println!("\n=== EXECUTION CACHE LOCK CONTENTION COMPARISON ===");
        println!("Validation delay: {:?}", validation_delay);
        println!("Concurrent readers: {}", num_readers);
        println!();
        println!("OLD (recv inside lock):  max reader wait = {:?}", old_max);
        for (i, w) in old_reader_waits.iter().enumerate() {
            println!("  reader {}: {:?}", i, w);
        }
        println!();
        println!("NEW (recv outside lock): max reader wait = {:?}", new_max);
        for (i, w) in new_reader_waits.iter().enumerate() {
            println!("  reader {}: {:?}", i, w);
        }
        println!();
        println!(
            "Speedup: {:.0}x",
            old_max.as_nanos() as f64 / new_max.as_nanos().max(1) as f64
        );

        assert!(
            old_max >= &Duration::from_millis(80),
            "OLD behavior should block readers for ~100ms"
        );
        assert!(
            new_max < &Duration::from_millis(5),
            "NEW behavior should not block readers"
        );
    }
}
