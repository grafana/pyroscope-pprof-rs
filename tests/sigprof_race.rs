// Regression test for the SIGPROF race condition fixed in:
// https://github.com/grafana/pprof-rs/commit/978d3aa248fa19be6cc6f8488f1472cea98bf8a2
//
// The bug: unregister_signal_handler() restored the previous sigaction (SIG_DFL).
// SIGPROF's default action is to terminate the process. If a pending SIGPROF is
// delivered in the window between unregistering the handler and re-registering it
// (during rapid start/stop cycles), the process crashes.
//
// Run with:
//   cargo test --test sigprof_race -- --test-threads 1
//
// Without the fix, this test crashes the process with SIGPROF.
// With the fix (SIG_IGN instead of SIG_DFL restore), it completes cleanly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn test_sigprof_race_crash() {
    // Spawn background threads that burn CPU to maximize SIGPROF delivery.
    // SIGPROF is delivered based on CPU time consumed by the process, so
    // more threads burning CPU = more frequent signal delivery = wider
    // effective race window.
    let running = Arc::new(AtomicBool::new(true));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let running = running.clone();
        handles.push(std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                std::hint::black_box(0u64.wrapping_add(1));
            }
        }));
    }

    for _ in 0..8000 {
        let guard = pprof::ProfilerGuard::new(999).unwrap();
        // Minimal busy-loop: just enough to ensure some SIGPROF signals fire.
        // guard is dropped at end of iteration, cycling through the race window.
        let start = std::time::Instant::now();
        while start.elapsed().as_micros() < 500 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
    }

    running.store(false, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
}
