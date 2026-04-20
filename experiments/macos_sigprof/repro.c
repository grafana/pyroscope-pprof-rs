/*
 * experiment 9: pure-libc SIGPROF reproducer for the macOS SIGBUS crash
 * observed in the Rust sigprof_race test. No Rust, no framehop, no unwinding.
 *
 * Build knobs (compile-time macros):
 *   STACK_SIZE    pthread worker stack size in bytes    (default 2*1024*1024)
 *   ITERATIONS    number of main-loop iterations        (default 8000)
 *   MODE          RACE or STEADY                         (default RACE)
 *     RACE   -> sigaction(SIGPROF, ...) register/unregister churn every ~1 ms
 *     STEADY -> install SIGPROF handler once, just sleep
 *
 * On crash the SIGBUS/SIGSEGV handler prints:
 *   signo, si_code, si_errno, si_addr,
 *   rip, rsp, rbp, trapno, err, faultvaddr,
 *   stack_base (high end), stack_size, rsp_distance_from_high = stack_base - rsp
 * then _exit(1).
 * On success prints `OK iterations=N stack_size=S` and _exit(0).
 */

#define _DARWIN_C_SOURCE
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#ifndef STACK_SIZE
#define STACK_SIZE (2 * 1024 * 1024)
#endif

#ifndef ITERATIONS
#define ITERATIONS 8000
#endif

#define MODE_RACE   1
#define MODE_STEADY 2

#ifndef MODE
#define MODE MODE_RACE
#endif

#ifndef NWORKERS
#define NWORKERS 4
#endif

#define ALTSTACK_SIZE (SIGSTKSZ * 4)

/* ------------------------------------------------------------------- */
/* async-signal-safe printing                                          */
/* ------------------------------------------------------------------- */

static void safe_write(const char *buf, size_t n) {
    ssize_t off = 0;
    while ((size_t)off < n) {
        ssize_t w = write(2, buf + off, n - (size_t)off);
        if (w <= 0) return;  /* bail on error/EOF; we are exiting anyway */
        off += w;
    }
}

/* snprintf is not strictly AS-safe on all libcs, but on Darwin for simple
 * %s/%lx/%d conversions (no locale-dependent floats) it has been acceptable
 * in many low-level programs. We are calling _exit(1) immediately after, so
 * any re-entrancy damage is irrelevant. */
static void safe_kv_hex(const char *key, uint64_t v) {
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "  %s = 0x%016llx\n", key,
                     (unsigned long long)v);
    if (n > 0) safe_write(buf, (size_t)n);
}

static void safe_kv_dec(const char *key, long long v) {
    char buf[128];
    int n = snprintf(buf, sizeof(buf), "  %s = %lld\n", key, v);
    if (n > 0) safe_write(buf, (size_t)n);
}

static void safe_line(const char *s) {
    safe_write(s, strlen(s));
}

/* ------------------------------------------------------------------- */
/* fault handler                                                       */
/* ------------------------------------------------------------------- */

#include <sys/ucontext.h>

static void fault_handler(int signo, siginfo_t *si, void *ucv) {
    ucontext_t *uc = (ucontext_t *)ucv;

    const char *name = "UNKNOWN";
    if (signo == SIGBUS)  name = "SIGBUS";
    if (signo == SIGSEGV) name = "SIGSEGV";
    if (signo == SIGILL)  name = "SIGILL";

    safe_line("=== fault caught ===\n");
    {
        char buf[64];
        int n = snprintf(buf, sizeof(buf), "  signal = %s (%d)\n", name, signo);
        if (n > 0) safe_write(buf, (size_t)n);
    }
    safe_kv_dec("si_code",  si ? (long long)si->si_code  : -1);
    safe_kv_dec("si_errno", si ? (long long)si->si_errno : -1);
    safe_kv_hex("si_addr",  si ? (uint64_t)(uintptr_t)si->si_addr : 0);

#if defined(__x86_64__)
    if (uc && uc->uc_mcontext) {
        safe_kv_hex("rip",        (uint64_t)uc->uc_mcontext->__ss.__rip);
        safe_kv_hex("rsp",        (uint64_t)uc->uc_mcontext->__ss.__rsp);
        safe_kv_hex("rbp",        (uint64_t)uc->uc_mcontext->__ss.__rbp);
        safe_kv_hex("trapno",     (uint64_t)uc->uc_mcontext->__es.__trapno);
        safe_kv_hex("err",        (uint64_t)uc->uc_mcontext->__es.__err);
        safe_kv_hex("faultvaddr", (uint64_t)uc->uc_mcontext->__es.__faultvaddr);

        pthread_t self = pthread_self();
        void   *hi   = pthread_get_stackaddr_np(self);
        size_t  sz   = pthread_get_stacksize_np(self);
        uint64_t rsp = (uint64_t)uc->uc_mcontext->__ss.__rsp;

        safe_kv_hex("stack_base_high", (uint64_t)(uintptr_t)hi);
        safe_kv_hex("stack_size",      (uint64_t)sz);
        safe_kv_hex("stack_low",       (uint64_t)((uintptr_t)hi - sz));
        safe_kv_hex("rsp_distance_from_high",
                    (uint64_t)((uintptr_t)hi - (uintptr_t)rsp));
    } else {
        safe_line("  (no ucontext / no mcontext)\n");
    }
#else
    safe_line("  (non-x86_64 build; mcontext dump omitted)\n");
#endif

    safe_line("=== end fault ===\n");
    _exit(1);
}

/* ------------------------------------------------------------------- */
/* SIGPROF handler: true no-op (SA_SIGINFO to match pprof-rs)          */
/* ------------------------------------------------------------------- */
static void sigprof_handler(int signo, siginfo_t *si, void *ucv) {
    (void)signo; (void)si; (void)ucv;
}

/* ------------------------------------------------------------------- */
/* worker: pure CPU loop, always eligible for SIGPROF delivery         */
/* ------------------------------------------------------------------- */
static void *worker(void *arg) {
    (void)arg;
    volatile uint64_t acc = 0;
    for (;;) {
        for (int i = 0; i < 1000000; i++) {
            acc += (uint64_t)i * (uint64_t)i;
        }
    }
    return NULL;
}

/* ------------------------------------------------------------------- */
/* main thread: drive the itimer + sigaction churn                     */
/* ------------------------------------------------------------------- */
int main(void) {
    /* install per-thread sigaltstack for the main thread. Workers also
     * get their own below. We want SA_ONSTACK for the fault handler so
     * a stack-overflow fault can still print. */
    stack_t altstack;
    altstack.ss_sp    = malloc(ALTSTACK_SIZE);
    altstack.ss_size  = ALTSTACK_SIZE;
    altstack.ss_flags = 0;
    if (!altstack.ss_sp) { perror("malloc altstack"); return 2; }
    if (sigaltstack(&altstack, NULL) != 0) { perror("sigaltstack"); return 2; }

    struct sigaction fa;
    memset(&fa, 0, sizeof(fa));
    fa.sa_sigaction = fault_handler;
    fa.sa_flags = SA_SIGINFO | SA_ONSTACK;
    sigemptyset(&fa.sa_mask);
    if (sigaction(SIGBUS,  &fa, NULL) != 0) { perror("sigaction SIGBUS");  return 2; }
    if (sigaction(SIGSEGV, &fa, NULL) != 0) { perror("sigaction SIGSEGV"); return 2; }
    if (sigaction(SIGILL,  &fa, NULL) != 0) { perror("sigaction SIGILL");  return 2; }

    struct sigaction pa;
    memset(&pa, 0, sizeof(pa));
    pa.sa_sigaction = sigprof_handler;
    pa.sa_flags = SA_SIGINFO;
    sigemptyset(&pa.sa_mask);

#if MODE == MODE_RACE
    const char *mode_str = "RACE";
#else
    const char *mode_str = "STEADY";
    if (sigaction(SIGPROF, &pa, NULL) != 0) { perror("sigaction SIGPROF"); return 2; }
#endif

    /* spawn workers with explicit stack size */
    pthread_t tids[NWORKERS];
    for (int i = 0; i < NWORKERS; i++) {
        pthread_attr_t attr;
        pthread_attr_init(&attr);
        if (pthread_attr_setstacksize(&attr, STACK_SIZE) != 0) {
            perror("pthread_attr_setstacksize");
            return 2;
        }
        if (pthread_create(&tids[i], &attr, worker, NULL) != 0) {
            perror("pthread_create");
            return 2;
        }
        pthread_attr_destroy(&attr);
    }

    /* 1 ms ITIMER_PROF (what pprof-rs uses) */
    struct itimerval it;
    it.it_interval.tv_sec = 0;
    it.it_interval.tv_usec = 1000;
    it.it_value.tv_sec = 0;
    it.it_value.tv_usec = 1000;
    if (setitimer(ITIMER_PROF, &it, NULL) != 0) {
        perror("setitimer");
        return 2;
    }

    {
        char buf[256];
        int n = snprintf(buf, sizeof(buf),
            "starting: mode=%s stack_size=%d iterations=%d nworkers=%d\n",
            mode_str, (int)STACK_SIZE, (int)ITERATIONS, NWORKERS);
        if (n > 0) safe_write(buf, (size_t)n);
    }

    /* main loop */
    for (int i = 0; i < ITERATIONS; i++) {
#if MODE == MODE_RACE
        struct sigaction old;
        if (sigaction(SIGPROF, &pa, &old) != 0) {
            perror("sigaction SIGPROF set");
            return 2;
        }
        usleep(500);
        if (sigaction(SIGPROF, &old, NULL) != 0) {
            perror("sigaction SIGPROF restore");
            return 2;
        }
        usleep(500);
#else
        usleep(1000);
#endif
        if (i % 500 == 0) {
            char buf[64];
            int n = snprintf(buf, sizeof(buf), "  iter=%d\n", i);
            if (n > 0) safe_write(buf, (size_t)n);
        }
    }

    {
        char buf[128];
        int n = snprintf(buf, sizeof(buf),
                         "OK iterations=%d stack_size=%d mode=%s\n",
                         (int)ITERATIONS, (int)STACK_SIZE, mode_str);
        if (n > 0) safe_write(buf, (size_t)n);
    }
    _exit(0);
}
