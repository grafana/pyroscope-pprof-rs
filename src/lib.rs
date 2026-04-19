// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.


/// Define the MAX supported stack depth. TODO: make this variable mutable.
#[cfg(feature = "large-depth")]
pub const MAX_DEPTH: usize = 1024;

#[cfg(all(feature = "huge-depth", not(feature = "large-depth")))]
pub const MAX_DEPTH: usize = 512;

#[cfg(not(any(feature = "large-depth", feature = "huge-depth")))]
pub const MAX_DEPTH: usize = 128;

/// Define the MAX supported thread name length. TODO: make this variable mutable.
pub const MAX_THREAD_NAME: usize = 16;

// todo replace with kindasafe
mod addr_validate;

mod backtrace;
mod collector;
mod error;
mod frames;
#[cfg(feature = "perfmaps")]
mod perfmap;
mod profiler;
mod report;
mod timer;

pub use self::addr_validate::validate;
pub use self::collector::{Collector, HashCounter};
pub use self::error::{Error, Result};
pub use self::frames::{Frames, Symbol};
pub use self::profiler::{ProfilerGuard, ProfilerGuardBuilder};
pub use self::report::{Report, ReportBuilder, UnresolvedReport};


#[allow(clippy::all)]
#[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
pub mod protos {
    pub use prost::Message;

    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/proto/perftools.profiles.rs"
    ));
}

#[cfg(feature = "protobuf-codec")]
pub mod protos {
    pub use protobuf::Message;

    include!(concat!(env!("OUT_DIR"), "/mod.rs"));

    pub use self::profile::*;
}

