//! An application that tracks speedruns of either single segments or (possibly
//! nested) groups of segments. Use `srtim add` to add segments or groups to
//! the config. Use `srtim run` to interactively run segments or groups using
//! keyboard input.
//!
//! See the README.md file at https://github.com/ktausch/srtim for more details
//! about the package as a whole and run `srtim help` to view the help message
//! indicating optional and required arguments for each subcommand.
use std::env;

use srtim::{Config, Input, Result, run_application};

/// Runs the application. See the help message for details on usage.
fn main() -> Result<()> {
    let mut input = Input::collect(env::args())?;
    let config = Config::load(input.extract_root(env::var("HOME").ok())?)?;
    run_application(input, config)
}
