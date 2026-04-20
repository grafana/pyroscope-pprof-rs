# Restrict `sigprof_race` test to the framehop unwinder

## Objective

Make `tests/sigprof_race.rs` compile and run **only** when the
`framehop-unwinder` feature is enabled. Under the default `backtrace-rs`
unwinder and the `frame-pointer` unwinder, the test must not be compiled
at all. CI must continue to exercise the framehop configuration so the
test actually runs on every push.

## Rationale

The `sigprof_race` test guards against regressions of the SIGPROF
signal-handler registration race (fix commit `978d3aa`). That race lives
in `src/profiler.rs:508-542`, which is unwinder-independent — so any one
unwinder is sufficient to catch a regression. Issue #28 shows the test
currently crashes on macOS with SIGBUS when built with the default
`backtrace-rs` unwinder (which on Darwin routes through libunwind.dylib
and dyld locks — not async-signal-safe). Framehop (`src/backtrace/framehop_unwinder.rs`)
unwinds from the `ucontext_t` directly, does not touch dyld, and is the
unwinder this project is prepared to call async-signal-safely. Running
the regression test **only** under framehop isolates the single concern
the test is designed to detect and unblocks macOS CI.

## Scope and assumptions

- Test location: `tests/sigprof_race.rs:1-53` (integration test binary).
- Unwinder dispatch: `src/backtrace/mod.rs:54-108`. Framehop is selected
  under
  `all(feature = "framehop-unwinder", any(target_arch = "x86_64", target_arch = "aarch64"), any(target_os = "linux", target_os = "macos"))`.
  Under any other configuration, the feature flag alone is not enough —
  backtrace-rs is picked instead.
- CI test jobs today (`.github/workflows/rust.yml:136-143`) run three
  `cargo test` invocations per matrix cell:
  1. `flamegraph,prost-codec` — default unwinder is backtrace-rs.
  2. `flamegraph,protobuf-codec` — default unwinder is backtrace-rs.
  3. `flamegraph,protobuf-codec,framehop-unwinder` — unwinder is framehop.
  The test matrix targets (`x86_64-unknown-linux-gnu`,
  `x86_64-unknown-linux-musl`, `x86_64-apple-darwin`) all satisfy the
  framehop cfg guard. No target currently in the test matrix would
  enable `framehop-unwinder` the feature but still pick backtrace-rs the
  impl.
- `frame-pointer` is never enabled in any CI test job, so it is relevant
  only for local developer builds.

## Implementation Plan

- [ ] Task 1. Add a `[[test]]` entry to `Cargo.toml` that pins the
      integration binary to `tests/sigprof_race.rs` and declares
      `required-features = ["framehop-unwinder"]`. Cargo's contract for
      `required-features` is to skip both build and execution of the
      target when any listed feature is missing, without emitting a
      warning. This is the same mechanism already used for the examples
      section at `Cargo.toml:71-89`. Concretely add, after the existing
      `[[bench]]` entries:
      ```
      [[test]]
      name = "sigprof_race"
      path = "tests/sigprof_race.rs"
      required-features = ["framehop-unwinder"]
      ```

- [ ] Task 2. Add a defense-in-depth crate-level attribute at the very
      top of `tests/sigprof_race.rs` that matches the exact framehop
      selection cfg from `src/backtrace/mod.rs:95-108`:
      ```
      #![cfg(all(
          feature = "framehop-unwinder",
          any(target_arch = "x86_64", target_arch = "aarch64"),
          any(target_os = "linux", target_os = "macos"),
      ))]
      ```
      Rationale: `required-features` in Task 1 handles the Cargo-level
      skip, but a user who enables `framehop-unwinder` on an
      unsupported target (e.g. `x86_64-unknown-linux-musl` is fine, but
      a hypothetical Windows or RISC-V 32 target) would still compile
      the test binary against an unwinder silently falling back to
      backtrace-rs. The crate-level `#![cfg(..)]` makes such a
      configuration produce an empty binary rather than a misleading
      pass/fail. On every CI row this attribute is true, so it is
      a no-op in practice.

- [ ] Task 3. Update the header comment in `tests/sigprof_race.rs:9-10`
      to reflect the required feature flag, replacing:
      ```
      // Run with:
      //   cargo test --test sigprof_race -- --test-threads 1
      ```
      with:
      ```
      // Run with:
      //   cargo test --features framehop-unwinder --test sigprof_race -- --test-threads 1
      ```
      Also add one sentence explaining why: "This test only compiles
      under the framehop unwinder because the default backtrace-rs
      unwinder on macOS has separate async-signal-safety issues that
      mask the signal-handler-registration race this test is meant to
      catch. See issue #28."

- [ ] Task 4. Confirm CI still exercises the framehop path on every
      test matrix cell. `.github/workflows/rust.yml:142-143` defines
      `Run cargo test framehop` with
      `--features flamegraph,protobuf-codec,framehop-unwinder` and is
      unconditional across the full matrix of (stable, nightly) ×
      (x86_64-unknown-linux-gnu, x86_64-unknown-linux-musl,
      x86_64-apple-darwin). No workflow change is required; this task
      is a read-only verification that runs during review.

- [ ] Task 5. Local verification on Linux (we have no macOS machine).
      Run each of the three CI invocations and confirm behavior:
      - `cargo test --features flamegraph,prost-codec -- --test-threads 1`
        → the `sigprof_race-*` binary must be absent from
        `target/debug/deps/` and no `test_sigprof_race_crash` line
        appears in test output.
      - `cargo test --features flamegraph,protobuf-codec -- --test-threads 1`
        → same expectations as above.
      - `cargo test --features flamegraph,protobuf-codec,framehop-unwinder -- --test-threads 1`
        → the binary is built, `test_sigprof_race_crash` runs, and
        passes (on Linux the test has 42/0 history per issue #28).

- [ ] Task 6. Open a draft PR with the three changes from Tasks 1–3
      batched into a single commit, titled `test: restrict sigprof_race
      to framehop-unwinder feature`. PR body should reference issue #28,
      summarise the rationale from this plan, and note that the CI
      `framehop` step on macOS is now the one authoritative run for the
      regression guard.

## Verification Criteria

- `grep -r "sigprof_race" target/debug/deps/` after step 1 (prost) and
  step 2 (protobuf) of a local CI reproduction prints nothing.
- The same grep after step 3 (framehop) prints the compiled binary
  path, and `cargo test --features ...framehop-unwinder sigprof_race`
  reports `test result: ok. 1 passed`.
- CI run on the PR shows the `sigprof_race` binary under the
  `Run cargo test framehop` step for every matrix cell, and shows no
  mention of it under `Run cargo test prost` or `Run cargo test
  protobuf`.
- If the `SIG_IGN` fix in `src/profiler.rs:528-542` is manually reverted
  to `SIG_DFL` on a temporary branch, the framehop step on Linux CI
  still crashes with SIGPROF — proving the regression guard is intact.
- No change in behaviour for consumers who build the crate with
  default features (no feature-flag churn, no new required dep, no new
  public API).

## Potential Risks and Mitigations

1. **Loss of SIG_DFL-regression coverage under the default unwinder.**
   After this change, the test only runs under framehop. A theoretical
   regression of the SIG_IGN fix that somehow only triggered under
   backtrace-rs would go undetected.
   Mitigation: the signal-handler registration code path
   (`src/profiler.rs:508-542`) is unwinder-agnostic — there is no path
   in that function that branches on the active unwinder. If stricter
   assurance is wanted later, add a lightweight unit test in
   `src/profiler.rs`'s `tests` module that calls `start`/`stop`
   repeatedly without spawning a timer, which is safe under every
   unwinder.

2. **The framehop test may still fail on macOS**, exposing a different
   bug (stack overflow in handler, or libmalloc reentry — see plan
   v1's H2/H3).
   Mitigation: this is the intended diagnostic outcome — framehop does
   not touch libunwind/dyld, so if the test still SIGBUSes we know the
   cause is not H1 and we continue the diagnosis from plan v1. No code
   change should be added preemptively.

3. **`required-features` is silent on unmatched features.** Cargo does
   not print a warning when a test is skipped due to missing features,
   so a developer running `cargo test --features prost-codec` and
   seeing no `sigprof_race` output may be surprised.
   Mitigation: the updated header comment (Task 3) makes the
   requirement explicit in the file, and the PR description documents
   it. This is the same trade-off the examples section already accepts.

4. **Cargo feature+target cfg skew.** If framehop support is later
   extended to a new platform but the `#![cfg(..)]` in Task 2 is not
   updated, the test silently stops compiling there.
   Mitigation: the `#![cfg(..)]` mirrors `src/backtrace/mod.rs:95-108`
   exactly. Add a short comment in both places
   (`// keep in sync with tests/sigprof_race.rs` and vice versa) so a
   future contributor extending platform support updates both.

## Alternative approaches

1. Gate only via `required-features` in `Cargo.toml` and omit the
   crate-level `#![cfg(..)]`. Simpler by one file; loses protection
   against the platform-mismatch edge case described in Risk 4.
   Acceptable if the simplicity is preferred; recommended only if the
   team decides that the CI matrix will always be a strict subset of
   framehop-supported targets.

2. Gate on `#[cfg(feature = "framehop-unwinder")]` placed on the `fn
   test_sigprof_race_crash` item instead of the crate. Rejected: the
   test crate still compiles, linking every transitive dep and
   emitting an empty binary; wastes CI time.

3. Build a dedicated CI job for framehop only (separate
   `strategy.include` row) instead of relying on the existing
   `Run cargo test framehop` step. Rejected as overengineering — the
   existing step already runs on every matrix cell.
