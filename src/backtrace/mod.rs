// Copyright 2022 TiKV Project Authors. Licensed under Apache-2.0.

#[cfg(not(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(target_os = "linux", target_os = "macos"),
)))]
compile_error!("pprof-rs requires x86_64 or aarch64 on Linux or macOS");

mod framehop_unwinder;

pub use framehop_unwinder::{Frame, Trace};
