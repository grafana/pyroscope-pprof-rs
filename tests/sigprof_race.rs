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

#[test]
fn test_sigprof_race_crash() {
    for _ in 0..500 {
        let guard = pprof::ProfilerGuard::new(999).unwrap();
        // Busy-loop to keep CPU time ticking so SIGPROF fires frequently.
        let start = std::time::Instant::now();
        while start.elapsed().as_millis() < 2 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        drop(guard);
        // No sleep between iterations: the race window is the gap between
        // drop(timer) and the next register_signal_handler() call.
    }
}
