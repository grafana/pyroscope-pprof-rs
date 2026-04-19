// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::collections::HashMap;
use std::fmt::{Debug, Formatter};

use spin::RwLock;

use crate::frames::{UnresolvedFrames};
use crate::profiler::Profiler;
use crate::timer::ReportTiming;

use crate::{Error, Result};


/// The presentation of an unsymbolicated report which is actually an `HashMap` from `UnresolvedFrames` to isize (count).
pub struct UnresolvedReport {
    /// key is a backtrace captured by profiler and value is count of it.
    pub data: HashMap<UnresolvedFrames, isize>,

    /// Collection frequency, start time, duration.
    pub timing: ReportTiming,
}


/// A builder of `Report` and `UnresolvedReport`. It builds report from a running `Profiler`.
pub struct ReportBuilder<'a> {
    profiler: &'a RwLock<Result<Profiler>>,
    timing: ReportTiming,
}

impl<'a> ReportBuilder<'a> {
    pub(crate) fn new(profiler: &'a RwLock<Result<Profiler>>, timing: ReportTiming) -> Self {
        Self {
            profiler,
            timing,
        }
    }

    // TODO pyroscope does not need deduplication twice (here and in the pprof builder)
    // TODO remove ReportBuilder all-together
    /// Build an `UnresolvedReport`
    pub fn build_unresolved_and_reset(&self) -> Result<UnresolvedReport> {
        let mut hash_map = HashMap::new();

        match self.profiler.write().as_mut() {
            Err(err) => {
                log::error!("Error in creating profiler: {}", err);
                Err(Error::CreatingError)
            }
            Ok(profiler) => {
                profiler.data.try_iter()?.for_each(|entry| {
                    let count = entry.count;
                    if count > 0 {
                        let key = &entry.item;
                        match hash_map.get_mut(key) {
                            Some(value) => {
                                *value += count;
                            }
                            None => {
                                match hash_map.insert(key.clone(), count) {
                                    None => {}
                                    Some(_) => {
                                        unreachable!();
                                    }
                                };
                            }
                        }
                    }
                });

                profiler.clear()?;

                Ok(UnresolvedReport {
                    data: hash_map,
                    timing: self.timing.clone(),
                })
            }
        }
    }
}

