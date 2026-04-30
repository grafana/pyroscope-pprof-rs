#![cfg(feature = "framehop-unwinder")]

// Regression test for SIGPROF races during rapid profiler start/stop cycles.
//
// Original bug (978d3aa): unregister_signal_handler() restored SIG_DFL, which
// terminates the process. Fixed by switching to SIG_IGN on non-macOS platforms.
//
// macOS 26 bug: on macOS 26 ARM64, the kernel passes SIG_IGN (value 1) directly
// to _sigtramp as the handler address. _sigtramp branches to 0x1, which is not
// instruction-aligned → SIGBUS. Fixed by using a real no-op handler on macOS.
// See tests/sigign_macos_crash.c for a standalone C reproducer.
//
// Run with:
//   cargo test --test sigprof_race -- --test-threads 1

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn test_sigprof_race_crash() {
    // Spawn background threads that burn CPU to maximize SIGPROF delivery.
    // ITIMER_PROF counts process-wide CPU time. More threads burning CPU
    // means more total CPU time consumed, which means SIGPROF fires more
    // frequently. The kernel delivers the signal to whichever thread is
    // currently running — some of those deliveries will hit the main thread
    // during the race window.
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

    // Rapidly cycle the profiler. Each iteration creates a guard (registers
    // signal handler, starts timer) and drops it (stops timer, unregisters
    // handler). The main thread burns CPU between cycles so SIGPROF can be
    // delivered to it during the race window between unregister and re-register.
    for j in 0..8000 {
        let _guard = pprof::ProfilerGuard::new(999).unwrap();
        for _ in 0..50_000 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        if j % 100 == 0 {
            println!(". ")
        }
    }

    running.store(false, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
}
