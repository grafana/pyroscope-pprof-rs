#![cfg(all(
    feature = "framehop-unwinder",
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "macos"),
))]

// Regression test for the SIGPROF race condition fixed in:
// https://github.com/grafana/pprof-rs/commit/978d3aa248fa19be6cc6f8488f1472cea98bf8a2
//
// The bug: unregister_signal_handler() restored the previous sigaction (SIG_DFL).
// SIGPROF's default action is to terminate the process. If a pending SIGPROF is
// delivered in the window between unregistering the handler and re-registering it
// (during rapid start/stop cycles), the process crashes.
//
// Run with:
//   cargo test --features framehop-unwinder --test sigprof_race -- --test-threads 1
//
// This test only compiles under the framehop unwinder. The default
// backtrace-rs unwinder on macOS has separate async-signal-safety issues
// (libunwind/dyld reentrancy) that mask the signal-handler-registration
// race this test is meant to catch. See
// https://github.com/grafana/pyroscope-pprof-rs/issues/28.
//
// Without the fix, this test crashes the process with SIGPROF.
// With the fix (SIG_IGN instead of SIG_DFL restore), it completes cleanly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// === Diagnostic SIGBUS/SIGSEGV capture (investigation, not a fix) ===
//
// Installed before the profiler ever starts. The handler writes raw bytes to
// stderr via libc::write (async-signal-safe) and then calls _exit(139).
mod sigbus_diag {
    use std::os::raw::c_int;

    // Hex-encode `val` (16 hex digits, big-endian) into `buf` at `off`.
    fn write_hex_u64(buf: &mut [u8], off: &mut usize, val: u64) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        buf[*off] = b'0';
        buf[*off + 1] = b'x';
        *off += 2;
        for i in (0..16).rev() {
            buf[*off] = HEX[((val >> (i * 4)) & 0xf) as usize];
            *off += 1;
        }
    }

    fn write_hex_u32(buf: &mut [u8], off: &mut usize, val: u32) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        buf[*off] = b'0';
        buf[*off + 1] = b'x';
        *off += 2;
        for i in (0..8).rev() {
            buf[*off] = HEX[((val >> (i * 4)) & 0xf) as usize];
            *off += 1;
        }
    }

    fn write_dec_i32(buf: &mut [u8], off: &mut usize, val: i32) {
        if val < 0 {
            buf[*off] = b'-';
            *off += 1;
            write_dec_u32(buf, off, (-(val as i64)) as u32);
        } else {
            write_dec_u32(buf, off, val as u32);
        }
    }

    fn write_dec_u32(buf: &mut [u8], off: &mut usize, val: u32) {
        if val == 0 {
            buf[*off] = b'0';
            *off += 1;
            return;
        }
        let mut digits = [0u8; 10];
        let mut n = 0;
        let mut v = val;
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            buf[*off] = digits[i];
            *off += 1;
        }
    }

    fn write_str(buf: &mut [u8], off: &mut usize, s: &[u8]) {
        for &b in s {
            buf[*off] = b;
            *off += 1;
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    unsafe fn extract_regs(ucontext: *mut libc::c_void) -> (u64, u64, u64, u32, u32, u64) {
        // (rip, rsp, rbp, trapno, err, faultvaddr)
        if ucontext.is_null() {
            return (0, 0, 0, 0, 0, 0);
        }
        let uc = ucontext as *mut libc::ucontext_t;
        let mc = (*uc).uc_mcontext;
        if mc.is_null() {
            return (0, 0, 0, 0, 0, 0);
        }
        let ss = &(*mc).__ss;
        let es = &(*mc).__es;
        (
            ss.__rip,
            ss.__rsp,
            ss.__rbp,
            es.__trapno as u32,
            es.__err,
            es.__faultvaddr,
        )
    }

    #[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
    unsafe fn extract_regs(_ucontext: *mut libc::c_void) -> (u64, u64, u64, u32, u32, u64) {
        (0, 0, 0, 0, 0, 0)
    }

    extern "C" fn handler(
        signo: c_int,
        siginfo: *mut libc::siginfo_t,
        ucontext: *mut libc::c_void,
    ) {
        // Pre-allocated buffer; do not allocate from a signal handler.
        let mut buf = [0u8; 2048];
        let mut off = 0usize;

        write_str(&mut buf, &mut off, b"\n=== SIGBUS CAPTURE ===\n");

        let (si_addr, si_code, si_errno) = unsafe {
            if siginfo.is_null() {
                (0u64, 0i32, 0i32)
            } else {
                (
                    (*siginfo).si_addr() as u64,
                    (*siginfo).si_code,
                    (*siginfo).si_errno,
                )
            }
        };

        write_str(&mut buf, &mut off, b"signo=");
        write_dec_i32(&mut buf, &mut off, signo);
        write_str(&mut buf, &mut off, b"  code=");
        write_dec_i32(&mut buf, &mut off, si_code);
        write_str(&mut buf, &mut off, b"  errno=");
        write_dec_i32(&mut buf, &mut off, si_errno);
        write_str(&mut buf, &mut off, b"\n");

        write_str(&mut buf, &mut off, b"si_addr=");
        write_hex_u64(&mut buf, &mut off, si_addr);
        write_str(&mut buf, &mut off, b"\n");

        let (rip, rsp, rbp, trapno, err, faultvaddr) = unsafe { extract_regs(ucontext) };
        write_str(&mut buf, &mut off, b"rip=");
        write_hex_u64(&mut buf, &mut off, rip);
        write_str(&mut buf, &mut off, b"  rsp=");
        write_hex_u64(&mut buf, &mut off, rsp);
        write_str(&mut buf, &mut off, b"  rbp=");
        write_hex_u64(&mut buf, &mut off, rbp);
        write_str(&mut buf, &mut off, b"\n");

        write_str(&mut buf, &mut off, b"trapno=");
        write_hex_u32(&mut buf, &mut off, trapno);
        write_str(&mut buf, &mut off, b"  err=");
        write_hex_u32(&mut buf, &mut off, err);
        write_str(&mut buf, &mut off, b"  faultvaddr=");
        write_hex_u64(&mut buf, &mut off, faultvaddr);
        write_str(&mut buf, &mut off, b"\n");

        // Thread / stack info (macOS has pthread_get_stackaddr_np /
        // pthread_get_stacksize_np; Linux uses pthread_attr_getstack).
        let pself = unsafe { libc::pthread_self() } as u64;

        #[cfg(target_os = "macos")]
        let (stack_base, stack_size) = unsafe {
            extern "C" {
                fn pthread_get_stackaddr_np(thread: libc::pthread_t) -> *mut libc::c_void;
                fn pthread_get_stacksize_np(thread: libc::pthread_t) -> libc::size_t;
            }
            let addr = pthread_get_stackaddr_np(libc::pthread_self()) as u64;
            let size = pthread_get_stacksize_np(libc::pthread_self()) as u64;
            (addr, size)
        };

        #[cfg(target_os = "linux")]
        let (stack_base, stack_size) = (0u64, 0u64);

        let rsp_distance_from_base = stack_base.wrapping_sub(rsp);

        write_str(&mut buf, &mut off, b"pthread_self=");
        write_hex_u64(&mut buf, &mut off, pself);
        write_str(&mut buf, &mut off, b"\n");

        write_str(&mut buf, &mut off, b"stack_base=");
        write_hex_u64(&mut buf, &mut off, stack_base);
        write_str(&mut buf, &mut off, b"  stack_size=");
        write_hex_u64(&mut buf, &mut off, stack_size);
        write_str(&mut buf, &mut off, b"  rsp_distance_from_base=");
        write_hex_u64(&mut buf, &mut off, rsp_distance_from_base);
        write_str(&mut buf, &mut off, b"\n");

        write_str(&mut buf, &mut off, b"=== END SIGBUS CAPTURE ===\n");

        unsafe {
            libc::write(2, buf.as_ptr() as *const libc::c_void, off);
            libc::_exit(139);
        }
    }

    pub fn install() {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handler as usize;
            sa.sa_flags = libc::SA_SIGINFO;
            libc::sigemptyset(&mut sa.sa_mask);
            // NOTE: intentionally NOT using SA_ONSTACK here — we want to observe
            // the natural behavior of the original crash.
            libc::sigaction(libc::SIGBUS, &sa, std::ptr::null_mut());
            libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
        }
    }
}

#[test]
fn test_sigprof_race_crash() {
    // Install diagnostic fault handlers BEFORE any profiler work.
    sigbus_diag::install();

    // Spawn background threads that burn CPU to maximize SIGPROF delivery.
    // ITIMER_PROF counts process-wide CPU time. More threads burning CPU
    // means more total CPU time consumed, which means SIGPROF fires more
    // frequently. The kernel delivers the signal to whichever thread is
    // currently running — some of those deliveries will hit the main thread
    // during the race window.
    let running = Arc::new(AtomicBool::new(true));
    let mut handles = Vec::new();
    // === EXPERIMENT 4 (investigation, not a fix) ===
    // Drop burner threads from 4 to 0. SIGPROF (ITIMER_PROF) will now only
    // hit the main thread (8 MB stack), not a secondary pthread (2 MB stack).
    // Previous captures all showed the fault on a 2 MB pthread with
    // pthread_self == stack_base. If SIGBUS disappears: the bug is specific
    // to SIGPROF delivery to a secondary pthread. If it persists: the bug
    // also manifests on the main thread.
    for _ in 0..0 {
        let running = running.clone();
        handles.push(std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                std::hint::black_box(0u64.wrapping_add(1));
            }
        }));
    }

    // === EXPERIMENT 3 (investigation, not a fix) ===
    // Hoist the ProfilerGuard out of the loop so that register_signal_handler
    // / unregister_signal_handler are each called exactly ONCE across the
    // lifetime of the test. This removes the sigaction-churn race entirely.
    //
    // If SIGBUS still reproduces: the bug is NOT the sigaction register/
    // unregister race — it's something in the steady-state signal delivery.
    // If SIGBUS disappears: the race is the cause.
    let _guard = pprof::ProfilerGuard::new(999).unwrap();
    for _ in 0..8000 {
        for _ in 0..50_000 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
    }
    drop(_guard);

    running.store(false, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
}
