// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::fmt::{self, Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::time::SystemTime;

use smallvec::SmallVec;

use crate::backtrace::{Frame, Trace, TraceImpl};
use crate::{MAX_DEPTH, MAX_THREAD_NAME};

#[derive(Clone)]
pub struct UnresolvedFrames {
    pub frames: SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]>,
    pub thread_name: [u8; MAX_THREAD_NAME],
    pub thread_name_length: usize,
    pub thread_id: u64,
    pub sample_timestamp: SystemTime,
}

impl Default for UnresolvedFrames {
    fn default() -> Self {
        let frames = SmallVec::with_capacity(MAX_DEPTH);
        Self {
            frames,
            thread_name: [0; MAX_THREAD_NAME],
            thread_name_length: 0,
            thread_id: 0,
            sample_timestamp: SystemTime::now(),
        }
    }
}

impl Debug for UnresolvedFrames {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        self.frames.fmt(f)
    }
}

impl UnresolvedFrames {
    pub fn new(
        frames: SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]>,
        tn: &[u8],
        thread_id: u64,
        sample_timestamp: SystemTime,
    ) -> Self {
        let thread_name_length = tn.len();
        let mut thread_name = [0; MAX_THREAD_NAME];
        thread_name[0..thread_name_length].clone_from_slice(tn);

        Self {
            frames,
            thread_name,
            thread_name_length,
            thread_id,
            sample_timestamp,
        }
    }
}

impl PartialEq for UnresolvedFrames {
    fn eq(&self, other: &Self) -> bool {
        let (frames1, frames2) = (&self.frames, &other.frames);
        if self.thread_id != other.thread_id || frames1.len() != frames2.len() {
            false
        } else {
            Iterator::zip(frames1.iter(), frames2.iter())
                .all(|(s1, s2)| s1.symbol_address() == s2.symbol_address())
        }
    }
}

impl Eq for UnresolvedFrames {}

impl Hash for UnresolvedFrames {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.frames
            .iter()
            .for_each(|frame| frame.symbol_address().hash(state));
        self.thread_id.hash(state);
    }
}

