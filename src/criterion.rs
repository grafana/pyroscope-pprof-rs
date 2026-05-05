// Copyright 2020 TiKV Project Authors. Licensed under Apache-2.0.

#[cfg(feature = "_protobuf")]
use crate::protos::Message;

use crate::ProfilerGuard;
use criterion::profiler::Profiler;

use std::fs::File;
#[cfg(feature = "_protobuf")]
use std::io::Write;
use std::os::raw::c_int;
use std::path::Path;

#[allow(clippy::large_enum_variant)]
pub enum Output {
    #[cfg(feature = "_protobuf")]
    Protobuf,
}

pub struct PProfProfiler<'a> {
    frequency: c_int,
    output: Output,
    active_profiler: Option<ProfilerGuard<'a>>,
}

impl<'a> PProfProfiler<'a> {
    pub fn new(frequency: c_int, output: Output) -> Self {
        Self {
            frequency,
            output,
            active_profiler: None,
        }
    }
}

#[cfg(not(feature = "_protobuf"))]
compile_error!("Feature \"protobuf\" must be enabled when \"criterion\" feature is enabled.");

impl<'a> Profiler for PProfProfiler<'a> {
    fn start_profiling(&mut self, _benchmark_id: &str, _benchmark_dir: &Path) {
        self.active_profiler = Some(ProfilerGuard::new(self.frequency).unwrap());
    }

    fn stop_profiling(&mut self, _benchmark_id: &str, benchmark_dir: &Path) {
        std::fs::create_dir_all(benchmark_dir).unwrap();

        let filename = match self.output {
            #[cfg(feature = "_protobuf")]
            Output::Protobuf => "profile.pb",
        };
        let output_path = benchmark_dir.join(filename);
        let output_file = File::create(&output_path).unwrap_or_else(|_| {
            panic!("File system error while creating {}", output_path.display())
        });

        if let Some(profiler) = self.active_profiler.take() {
            match &mut self.output {
                #[cfg(feature = "_protobuf")]
                Output::Protobuf => {
                    let mut output_file = output_file;

                    let profile = profiler.report().build().unwrap().pprof().unwrap();

                    let mut content = Vec::new();
                    #[cfg(not(feature = "protobuf-codec"))]
                    profile
                        .encode(&mut content)
                        .expect("Error while encoding protobuf");
                    #[cfg(feature = "protobuf-codec")]
                    profile
                        .write_to_vec(&mut content)
                        .expect("Error while encoding protobuf");

                    output_file
                        .write_all(&content)
                        .expect("Error while writing protobuf");
                }
            }
        }
    }
}
