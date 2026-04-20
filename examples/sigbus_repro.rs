// Standalone reproducer for the macOS-only SIGBUS crash reported in
// https://github.com/grafana/pprof-rs/issues/28
//
// Runs the same workload as tests/sigprof_race.rs but as a plain binary so we
// can install a fatal-signal handler that captures a stacktrace at SIGBUS
// before the process is torn down.
//
// Usage:
//   cargo run --example sigbus_repro --target x86_64-apple-darwin

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

extern "C" {
    fn backtrace(array: *mut *mut libc::c_void, size: libc::c_int) -> libc::c_int;
    fn backtrace_symbols_fd(array: *const *mut libc::c_void, size: libc::c_int, fd: libc::c_int);
}

fn write_all(fd: libc::c_int, buf: &[u8]) {
    let mut p = buf.as_ptr();
    let mut left = buf.len();
    while left > 0 {
        let n = unsafe { libc::write(fd, p as *const libc::c_void, left) };
        if n <= 0 {
            return;
        }
        p = unsafe { p.add(n as usize) };
        left -= n as usize;
    }
}

fn write_hex(fd: libc::c_int, val: u64) {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in 0..16 {
        let nibble = ((val >> ((15 - i) * 4)) & 0xf) as u8;
        buf[2 + i] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        };
    }
    write_all(fd, &buf);
}

fn write_dec(fd: libc::c_int, mut val: u64) {
    if val == 0 {
        write_all(fd, b"0");
        return;
    }
    let mut tmp = [0u8; 20];
    let mut n = 0;
    while val > 0 {
        tmp[n] = b'0' + (val % 10) as u8;
        val /= 10;
        n += 1;
    }
    let mut out = [0u8; 20];
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    write_all(fd, &out[..n]);
}

#[allow(unused_variables)]
extern "C" fn fatal_signal_handler(
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ucontext: *mut libc::c_void,
) {
    const STDERR: libc::c_int = 2;
    write_all(STDERR, b"\n=== fatal signal ");
    write_dec(STDERR, sig as u64);
    write_all(STDERR, b" captured ===\n");

    if !info.is_null() {
        write_all(STDERR, b"si_addr=");
        let si_addr = unsafe { (*info).si_addr() } as usize as u64;
        write_hex(STDERR, si_addr);
        write_all(STDERR, b" si_code=");
        let si_code = unsafe { (*info).si_code } as i64 as u64;
        write_hex(STDERR, si_code);
        write_all(STDERR, b"\n");
    }

    // Pull interrupted register state out of the ucontext. libbacktrace on
    // macOS stops walking at the signal trampoline so we need this to resume
    // the unwind on the other side of _sigtramp.
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let (pre_pc, pre_fp, pre_sp) = unsafe {
        if ucontext.is_null() {
            (0u64, 0u64, 0u64)
        } else {
            let uc = ucontext as *const libc::ucontext_t;
            let mcontext = (*uc).uc_mcontext;
            if mcontext.is_null() {
                (0, 0, 0)
            } else {
                let ss = &(*mcontext).__ss;
                (ss.__rip as u64, ss.__rbp as u64, ss.__rsp as u64)
            }
        }
    };
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let (pre_pc, pre_fp, pre_sp) = unsafe {
        if ucontext.is_null() {
            (0u64, 0u64, 0u64)
        } else {
            let uc = ucontext as *const libc::ucontext_t;
            let mcontext = (*uc).uc_mcontext;
            if mcontext.is_null() {
                (0, 0, 0)
            } else {
                let ss = &(*mcontext).__ss;
                (ss.__pc as u64, ss.__fp as u64, ss.__sp as u64)
            }
        }
    };
    #[cfg(not(target_os = "macos"))]
    let (pre_pc, pre_fp, pre_sp) = (0u64, 0u64, 0u64);

    write_all(STDERR, b"pre_signal pc=");
    write_hex(STDERR, pre_pc);
    write_all(STDERR, b" fp=");
    write_hex(STDERR, pre_fp);
    write_all(STDERR, b" sp=");
    write_hex(STDERR, pre_sp);
    write_all(STDERR, b"\n");

    let mut frames: [*mut libc::c_void; 128] = [std::ptr::null_mut(); 128];
    let n = unsafe { backtrace(frames.as_mut_ptr(), 128) };
    write_all(STDERR, b"backtrace(3) (");
    write_dec(STDERR, n as u64);
    write_all(STDERR, b" frames):\n");
    unsafe { backtrace_symbols_fd(frames.as_ptr(), n, STDERR) };

    // Manual frame-pointer walk starting from the pre-signal fp. Provides the
    // frames libbacktrace lost on the other side of the signal trampoline.
    if pre_fp != 0 {
        write_all(STDERR, b"\nmanual fp walk from pre-signal fp:\n");
        let mut manual: [*mut libc::c_void; 128] = [std::ptr::null_mut(); 128];
        manual[0] = pre_pc as *mut libc::c_void;
        let mut count: usize = 1;
        let mut fp = pre_fp as usize;
        while count < 128 && fp != 0 && (fp & 0x7) == 0 {
            let saved_fp_ptr = fp as *const usize;
            let ret_addr_ptr = (fp + std::mem::size_of::<usize>()) as *const usize;
            let ret_addr = unsafe { std::ptr::read_volatile(ret_addr_ptr) };
            let next_fp = unsafe { std::ptr::read_volatile(saved_fp_ptr) };
            if ret_addr == 0 {
                break;
            }
            manual[count] = ret_addr as *mut libc::c_void;
            count += 1;
            if next_fp <= fp || next_fp - fp > 0x100000 {
                break;
            }
            fp = next_fp;
        }
        write_all(STDERR, b"manual walk produced ");
        write_dec(STDERR, count as u64);
        write_all(STDERR, b" frames:\n");
        unsafe { backtrace_symbols_fd(manual.as_ptr(), count as libc::c_int, STDERR) };
    }

    write_all(STDERR, b"=== end fatal signal ===\n");
    unsafe { libc::_exit(128 + sig) };
}

fn install_crash_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = fatal_signal_handler as usize;
        sa.sa_flags = libc::SA_SIGINFO;
        libc::sigemptyset(&mut sa.sa_mask);
        for sig in &[libc::SIGBUS, libc::SIGSEGV, libc::SIGILL, libc::SIGABRT] {
            libc::sigaction(*sig, &sa, std::ptr::null_mut());
        }
    }
}

fn main() {
    install_crash_handlers();

    // Same workload as tests/sigprof_race.rs::test_sigprof_race_crash.
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

    for i in 0..8000 {
        let _guard = pprof::ProfilerGuard::new(999).unwrap();
        for _ in 0..50_000 {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        if i % 500 == 0 {
            write_all(2, b"iter ");
            write_dec(2, i as u64);
            write_all(2, b"\n");
        }
    }

    running.store(false, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    write_all(2, b"sigbus_repro: completed all iterations without a crash\n");
}
