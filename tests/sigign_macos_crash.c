/*
 * Reproducer for macOS 26 ARM64: SIG_IGN passed as handler to _sigtramp.
 *
 * On macOS 26, when the signal disposition is SIG_IGN (value 1), the kernel
 * passes that value directly to _sigtramp as the handler address instead of
 * silently discarding the signal. _sigtramp branches to address 0x1, which is
 * not instruction-aligned on ARM64, causing SIGBUS (PC alignment fault).
 *
 * Setup:
 *   Thread A  spins (receives signals)
 *   Thread B  continuously fires SIGPROF at Thread A via pthread_kill
 *   Main      cycles sigaction: SA_SIGINFO handler → SIG_IGN → repeat
 *
 * Expected outcome on macOS 26: process killed by SIGBUS before 10M iterations.
 * Expected outcome on Linux:    completes without crashing (SIG_IGN works correctly).
 */

#include <stdio.h>
#include <signal.h>
#include <pthread.h>
#include <stdatomic.h>
#include <string.h>

static atomic_int g_running = 1;
static atomic_int g_handler_calls = 0;
static pthread_t  g_target_thread;

static void perf_signal_handler(int sig, siginfo_t *info, void *ctx) {
    (void)sig; (void)info; (void)ctx;
    atomic_fetch_add(&g_handler_calls, 1);
}

static void install_real(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = perf_signal_handler;
    sa.sa_flags     = SA_SIGINFO | SA_RESTART;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGPROF, &sa, NULL);
}

static void install_ign(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = SIG_IGN;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGPROF, &sa, NULL);
}

static void *target_thread(void *arg) {
    (void)arg;
    while (atomic_load_explicit(&g_running, memory_order_relaxed))
        __asm__ volatile("" ::: "memory");
    return NULL;
}

static void *sender_thread(void *arg) {
    (void)arg;
    while (atomic_load_explicit(&g_running, memory_order_relaxed)) {
        pthread_kill(g_target_thread, SIGPROF);
        for (volatile int i = 0; i < 10; i++);
    }
    return NULL;
}

int main(void) {
    pthread_t sender;
    pthread_create(&g_target_thread, NULL, target_thread, NULL);
    pthread_create(&sender,          NULL, sender_thread, NULL);

    for (int j = 0; j < 10000000; j++) {
        install_real();
        for (volatile int k = 0; k < 10; k++);
        install_ign();
        for (volatile int k = 0; k < 10; k++);

        if (j % 500000 == 0) {
            printf(". j=%d calls=%d\n", j, atomic_load(&g_handler_calls));
            fflush(stdout);
        }
    }

    atomic_store(&g_running, 0);
    pthread_join(g_target_thread, NULL);
    pthread_join(sender, NULL);

    printf("survived 10M iterations — bug not triggered (%d handler calls)\n",
           atomic_load(&g_handler_calls));
    return 0;
}
