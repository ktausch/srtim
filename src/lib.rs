//! The `srtim` library represents and provides utilities to create
//! speedrun runs (with death tracking for video games).

mod application;
mod config;
mod error;
mod input;
mod segment_info;
mod segment_run;
mod utils;
pub use application::run_application;
pub use config::Config;
pub use error::{Error, Result};
pub use input::Input;
pub use segment_run::{
    MillisecondsSinceEpoch, SegmentRunEvent, SupplementedSegmentRun,
};
