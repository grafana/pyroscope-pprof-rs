# macOS CI SIGBUS Analysis — `sigprof_race` regression test

## Objective

Diagnose why `tests/sigprof_race.rs::test_sigprof_race_crash` crashes on macOS CI
with **SIGBUS (signal 10)** after the `SIG_IGN` fix (commit `978d3aa`) is
applied, while the same test passes reliably on Linux (42/42 runs).
Produce evidence-backed root-cause hypotheses plus a minimal investigation path,
given we have no macOS machine available and can only reason from source.

## Scope and Assumptions

- Repository: `grafana/pyroscope-pprof-rs`, default branch `main`.
- Failing jobs: `Test (macos-latest, stable, x86_64-apple-darwin)` and
  `Test (macos-latest, nightly, x86_64-apple-darwin)`.
- The failing invocation is `cargo test --features flamegraph,{prost-codec|protobuf-codec|framehop-unwinder} --target x86_64-apple-darwin -- --test-threads 1`
  from `.github/workflows/rust.yml:136-143`.
- The `frame-pointer` feature is **not** enabled in any CI test job, so
  `SA_ONSTACK` is never set on macOS and the signal runs on the current
  thread's own stack (`src/profiler.rs:512-521`).
- On x86_64 macOS, `backtrace::trace_unsynchronized` (the default unwinder,
  `src/backtrace/mod.rs:54-73` and `src/backtrace/backtrace_rs.rs:17-25`)
  dispatches to libunwind via `_Unwind_Backtrace`. When the `framehop-unwinder`
  feature is enabled, `TraceImpl` becomes the framehop variant instead — so
  the framehop job is a separately-flagged data point, not the same code.
- Branch creation, `gh` calls, and any log download must be performed by an
  implementation agent; the planning agent has no shell. The plan below
  encodes "fetch run X" as a task, not something this plan executed.

## Key evidence collected

- Issue `grafana/pyroscope-pprof-rs#28` body and comments:
  - With fix reverted (commit `dead34f`): crash is **signal 27 / SIGPROF** — as
    expected (the old bug).
  - With fix applied (commit `13150e3`, run
    [22701001347](https://github.com/grafana/pprof-rs/actions/runs/22701001347)):
    crash is **signal 10 / SIGBUS "access to undefined memory"** on both
    stable and nightly macOS jobs, very early (~5–6 s into the test, all
    13 unit tests already pass).
  - Linux CI: 42 consecutive passing runs.
- `tests/sigprof_race.rs:42-47` runs **8000** `ProfilerGuard::new(999)`
  start/stop cycles, each separated by a 50 000-iteration busy loop on the
  main thread, with **4 background CPU-burning threads**.
- `src/profiler.rs:496-506` calls `unregister_signal_handler()` and then
  `init()` inside `stop()`, while holding the `PROFILER` write lock.
- `src/profiler.rs:467-473` — `init()` does `self.data = Collector::new()?`,
  **reallocating the entire `Collector`** on every stop.
- `src/collector.rs:117-125` — `HashCounter::default` allocates
  `BUCKETS = 4096` buckets, each holding `Box<[Entry<UnresolvedFrames>; 4]>`.
  `UnresolvedFrames` (`src/frames.rs:35-54`) inlines a `SmallVec<[Frame; 128]>`
  plus `[u8; 16]`, so **per Collector: ~4096 × 4 × sizeof(UnresolvedFrames)
  ≈ tens of MB of heap allocations**, repeated 8000× ⇒ massive alloc/free
  churn under concurrent SIGPROF delivery.
- `src/collector.rs:158-180` — `TempFdArray::new` opens a `NamedTempFile`
  on every `Collector::new`, so 8000 temp files are created and unlinked.
- `src/profiler.rs:317-415` — `perf_signal_handler` is async-signal-unsafe:
  calls `SystemTime::now()` (line 386), `libc::pthread_self` + `pthread_getname_np`
  (lines 256, 405–409), `TraceImpl::trace` (line 387), then `profiler.sample(...)`
  which goes through `UnresolvedFrames::new`, `Collector::add`,
  `HashCounter::add` (uses `DefaultHasher` whose thread-local seed is lazily
  initialized via `arc4random` on macOS), and potentially
  `TempFdArray::flush_buffer` → `File::write_all`.
- `SA_ONSTACK` is only available via `frame-pointer` feature
  (`src/profiler.rs:513-521`), so on macOS x86_64 CI the handler runs on the
  interrupted thread's own stack — including the background CPU-burner
  threads with default Rust 2 MiB stacks.

## Ranked root-cause hypotheses

The following is ordered by my estimated probability given the SIGBUS
signature, the macOS-only occurrence, and the workload profile.

### H1 (high): libunwind/dyld reentrancy on macOS during rapid sigaction churn

`backtrace::trace_unsynchronized` ⇒ `_Unwind_Backtrace` on macOS acquires the
dyld image-list reader lock to walk `__unwind_info` (compact unwind) tables.
When SIGPROF is delivered to a thread that is *already inside* a dyld /
libsystem critical section — which is dramatically more likely under the
test's workload because `ProfilerGuard::new` → `NamedTempFile::new` → `open`
/ `unlink` and the `sigaction` syscall itself exercise lazy bindings — the
second entry into libunwind on that thread can read from a dyld data
structure that is transiently inconsistent or from a page whose backing
has been invalidated, producing SIGBUS (macOS uses SIGBUS, not SIGSEGV, for
many guard-page and invalid-file-backed-page faults).

Why macOS only: Linux libunwind/libgcc_s `_Unwind_Backtrace` has a global
mutex but no dyld equivalent; the corresponding access is covered by
`dl_iterate_phdr` which is safe from signal context in practice on glibc.
macOS has no such guarantee.

### H2 (high): SIGBUS from stack overflow in the signal handler on a CPU-burner thread

No alternate signal stack is installed (default features), so SIGPROF is
delivered on the interrupted thread's own stack. The handler's on-stack
footprint is significant:
- `SmallVec<[Frame; MAX_DEPTH=128]>` inline (`src/profiler.rs:382-383`)
  — on macOS `backtrace::Frame` is a trampoline frame wrapper; inline
  capacity × frame size easily exceeds 1 KB on the stack.
- `UnresolvedFrames` is constructed in the handler (`src/profiler.rs:412`,
  `src/frames.rs:62-81`), which embeds the same 128-slot SmallVec and a
  `[u8; 16]` thread-name array (~2 KB on stack).
- Underneath, `_Unwind_Backtrace` on macOS uses its own per-call stack
  buffers for the compact-unwind/DWARF fallback.

On Linux, glibc's 8 MiB default thread stack absorbs this; on macOS the
default non-main thread stack is **512 KiB** (`pthread_create`) or
**2 MiB** if created via Rust's `std::thread::spawn` (Rust sets a larger
default). However, the signal arrival can land deep inside libsystem code
with its own frames on top, and on macOS a guard-page cross produces
**SIGBUS**, matching the observed crash. This alone would explain why
Linux passes and macOS crashes, and why `SA_ONSTACK` (only available via
`frame-pointer`) would fix it.

### H3 (medium): Non-async-signal-safe allocator reentrance inside the signal handler

The handler's path `profiler.sample` → `Collector::add` → `HashCounter::add`
→ `Bucket::add` → `TempFdArray::push` → possibly `flush_buffer` →
`File::write_all` can run *while the main thread is inside
`Collector::new()`* allocating/freeing tens of MB in libmalloc zones. When
`Profiler::stop` drops the write lock and `ProfilerGuard::new` re-acquires
it, there is a (tiny but real) window where a SIGPROF handler is already
running on a background thread inside `Collector::add` and the main thread
then takes the lock and deallocates the entire `Collector` that the signal
handler still holds a `&mut Profiler` reference to through its `try_write`
guard. The `spin::RwLock::try_write` guard is **not** re-entrant across
threads, so this particular scenario is actually blocked, *but* a closely
related scenario exists:
- Background thread T1: enters handler, `try_write` succeeds, begins
  `TraceImpl::trace`.
- Main thread: blocks on `PROFILER.write()` in `ProfilerGuard::drop` because
  T1 holds write.
- T1 finishes handler, releases lock.
- Main thread acquires lock, calls `init()` which reallocates `data`.
- **A second SIGPROF** is immediately delivered to T1 (the kernel had
  another pending); T1 re-enters handler, `try_write` returns `None`
  because main now holds the lock — safe.

So the cross-thread race is actually protected by `try_write`. H3 would
only fire if `libmalloc` re-entrance occurred *within a single thread*
(e.g., signal delivered while the interrupted thread was inside
`free`/`malloc_zone_*`). On macOS, libmalloc is explicitly **not**
async-signal-safe; a reentry can produce SIGBUS when reading from a
scribbled malloc zone header. With 272 GB of lifetime alloc/free over the
test, the probability is non-trivial.

### H4 (medium): SIGBUS from `TempFdArray` file-backed access during stop

`TempFdArray` keeps the tempfile open in `self.file` but does not mmap it
(the `try_iter` path in `src/collector.rs:216-234` uses `read_exact` into
an `AVec`, not `mmap`). So a truncated-mmap SIGBUS is unlikely from this
code directly. However, `NamedTempFile` on macOS uses `open(O_TMPFILE)` or
`mkstemp` + `unlink`; the file is deleted but an open fd remains. Writing
to an unlinked file is fine, *but* if free-space pressure in
`$TMPDIR` (macOS runners: often small tmpfs) causes `write` to deliver
`SIGXFSZ`/`EDQUOT`, that would be `io::Error`, not SIGBUS. So this
hypothesis is weaker but can be falsified cheaply by checking for mmap
calls in transitive dependencies (e.g., `memmap2` via `framehop-unwinder`
feature — the framehop job does exercise mmap).

### H5 (low): `SystemTime::now()` in the signal handler is not AS-safe on macOS

Per the open tracking issue `#22`, `SystemTime::now()` in a signal handler
has uncertain AS-safety on macOS. On x86_64 macOS, Rust's std calls
`clock_gettime(CLOCK_REALTIME)`, which is generally vDSO-safe, but macOS
does not ship a vDSO equivalent; it uses a commpage-mapped fast path that
is technically reentrant. This is unlikely to produce SIGBUS directly
(more likely a deadlock or corrupted time), so this ranks low for the
specific signature, but is worth noting because it's a known smell.

### H6 (low): `sigaction` race on macOS x86_64 — delivery during handler swap

macOS's `sigaction` is atomic from the kernel's perspective, but the
previously-installed handler may be invoked once more for a signal that
was already dequeued into the thread's per-thread pending set before the
swap. If that invocation occurs with the new handler's flags but the old
handler pointer (or vice versa), the signal trampoline may jump to a
stale function pointer. I have not been able to reproduce this from
public bug reports, so it ranks low, but it is the hypothesis most
directly implied by the issue's bullet 2 ("`SIG_IGN` may not take effect
atomically on macOS").

## Implementation Plan

- [ ] Task 1. **Create the working branch.** Fetch `origin/main` fresh,
      then create and push branch `kk/macos-ci-sigbus-investigation`
      from the current `origin/main` tip. Rationale: per project
      guidelines, always fetch base from remote and use the `kk/` prefix.

- [ ] Task 2. **Archive the crash evidence.** From run
      [22701001347](https://github.com/grafana/pprof-rs/actions/runs/22701001347),
      download the stable and nightly macOS test logs and any available
      crash report / spindump artifacts; also download one fresh macOS
      failing run on `main` (re-run the CI if needed to capture a current
      log). Save them under a scratch directory for correlation. This
      step yields hard data (exit signal, "child killed by signal" line,
      Darwin crash log if produced) that disambiguates H1–H4.
      Rationale: we have only `(signal: 10, SIGBUS)` from the issue
      comment — the kernel-reported `si_addr` and faulting instruction
      pointer are what ranks the hypotheses.

- [ ] Task 3. **Confirm the CI job/feature matrix that crashes.** From the
      same run, enumerate which of the three `cargo test` invocations in
      `.github/workflows/rust.yml:136-143` produced SIGBUS:
      `prost-codec`, `protobuf-codec`, or `framehop-unwinder`. The
      framehop job uses a different unwinder and mmaps object files via
      `memmap2`; if only that job crashes, H4 rises sharply. If all
      three crash, H1/H2 are more likely.

- [ ] Task 4. **Static sanity pass on the handler**: re-read
      `src/profiler.rs:306-415` against the Linux-vs-macOS diffs in
      ucontext access (already handled at
      `src/profiler.rs:342-350`, `361-369`), `pthread_getname_np`
      (handled at `src/profiler.rs:253-261`), and confirm no further
      Linux-only assumptions. Rationale: rule out a pure
      platform-dispatch bug before pursuing the harder hypotheses.

- [ ] Task 5. **Test H2 (stack overflow)** without macOS access: make
      `SA_ONSTACK` the default on macOS in `register_signal_handler`
      (gated behind the platform, independent of `frame-pointer`), or
      alternatively reduce `MAX_DEPTH` to 32 and shrink
      `MAX_THREAD_NAME` used inside the handler's on-stack buffer. Push
      to a CI branch and observe whether the SIGBUS disappears.
      Rationale: cheapest falsifier; if it fixes the crash, H2 is
      confirmed and `SA_ONSTACK` should become the macOS default.
      Open question: the `backtrace-rs` unwinder does not consult the
      `ucontext` (it unwinds the current stack), so using `SA_ONSTACK`
      with it yields useless samples. The fix therefore has to either
      (a) accept sampling only the handler stack on macOS, (b) require
      a ucontext-aware unwinder on macOS, or (c) apply `SA_ONSTACK`
      only during the test to validate the theory. This plan recommends
      approach (c) for diagnosis.

- [ ] Task 6. **Test H1 (libunwind/dyld reentrancy)** by swapping the
      default unwinder on macOS for the `framehop-unwinder`
      (`src/backtrace/mod.rs:95-108`) in a one-off CI build and running
      the same test. Framehop does not call into libunwind/dyld and
      unwinds from the `ucontext`, so if the crash goes away, H1 is
      confirmed. If it persists only on the non-framehop build, the
      proper long-term fix is either to guard the default unwinder
      behind a "best-effort" AS-safe path on macOS or to ship framehop
      as the macOS default.

- [ ] Task 7. **Test H3 (libmalloc reentry)** by linking the test binary
      with `MALLOC_GUARD_EDGES=1` / `MallocStackLogging=1` on macOS CI
      (environment variables, no code change) and re-running. A
      recognizable "malloc: \*\*\* error for object" stderr line
      immediately before the SIGBUS implicates libmalloc re-entrance.
      Rationale: the macOS allocator has built-in detection for this
      class of failure and it costs us only an env var.

- [ ] Task 8. **Reduce the test's aggressiveness on macOS** as a
      temporary mitigation while the above investigations run. Options:
      `#[cfg(target_os = "macos")]`-gate a lower iteration count
      (e.g. 500) and lower frequency (e.g. 199 Hz); or mark the test
      `#[ignore]` on macOS with a comment linking to issue #28. This
      unblocks CI on `main` while the real fix is being designed.
      Rationale: the purpose of the test is to catch regressions of
      the original SIG_DFL race, which is already validated on Linux;
      it should not be permitted to mask other bugs as required CI
      failures indefinitely.

- [ ] Task 9. **Write a one-page design note** summarising which
      hypothesis the CI data lands on and proposing the permanent fix
      (likely: adopt `SA_ONSTACK` as the macOS default or fall back to
      framehop on macOS). Rationale: this is a planning-only task and
      should not introduce code changes beyond those above.

## Verification Criteria

- The CI branch created in Task 1 tracks the current `origin/main` tip
  exactly (no extra commits, identical SHA).
- Logs and, if available, `ReportCrash` spindumps for at least one
  SIGBUS failure are saved locally and referenced in issue #28 as a
  comment, enabling anyone with macOS access to jump in.
- After Task 5, Task 6, and Task 7 land as separate trial CI runs, each
  with a clear "variable changed" commit message, the hypothesis table
  in the final design note is updated with PASS / FAIL for each.
- The long-term fix, once chosen, is guarded behind a conditional that
  does not regress Linux performance or accuracy (verified by running
  the same `sigprof_race` test on Linux under the fix).

## Potential Risks and Mitigations

1. **"Fix" hides a real bug.**
   Marking the test `#[ignore]` on macOS (Task 8) to unblock CI is a
   temporary patch that risks the real bug (H1–H3) reaching users who
   use pprof-rs on macOS.
   Mitigation: require a tracking issue link in the `#[ignore]`
   attribute and a CODEOWNERS approval on Task 9 before merging Task 8
   to `main`.

2. **No macOS access slows hypothesis testing.**
   Every falsification round requires a CI push.
   Mitigation: batch the three diagnostic experiments (Task 5, Task 6,
   Task 7) into a single branch-per-experiment pattern so they run in
   parallel on the matrix. Leverage `workflow_dispatch` with a feature-
   flag input if the matrix becomes expensive.

3. **Test flakiness masks signal.**
   If SIGBUS is intermittent rather than deterministic, a single
   passing run under an experimental fix does not prove causation.
   Mitigation: in each experimental branch, wrap the test in a shell
   retry loop that runs it 10× and reports a pass only if all 10
   succeed. Cheap (~60 s per run).

4. **`SA_ONSTACK` with `backtrace-rs` unwinder produces nonsense
   samples.**
   If we adopt H2's fix unconditionally, pprof-rs on macOS without the
   `frame-pointer` or `framehop-unwinder` feature will unwind the
   signal stack, not the application stack, making profiles useless.
   Mitigation: pair Task 5's fix with a doc-level note requiring either
   `frame-pointer` or `framehop-unwinder` when used on macOS, or make
   framehop the default unwinder on macOS (separate Cargo feature
   decision).

## Alternative Approaches

1. **Remove the `sigprof_race` test entirely on macOS and rely on
   Linux for the regression guarantee.** Trade-off: simplest; loses
   macOS-specific coverage of any future macOS-only regression of the
   same bug.

2. **Re-run the original bug (without SIG_IGN) on a Darwin VM locally
   on a developer laptop** rather than CI. Trade-off: faster
   iteration, at the cost of one-off machine access; does not help in
   this planning context since we explicitly lack macOS access, but
   should be documented as the preferred long-term diagnostic path.

3. **Replace the single-binary test with a multi-process harness** that
   spawns a child, cycles the profiler, and `waitpid`s. The parent can
   distinguish SIGBUS (child) from SIGPROF (child) cleanly and emit a
   structured failure report. Trade-off: more complex but gives much
   better signal when reproducing locally and in CI.
