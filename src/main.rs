//! TODO
use std::env;

use srtim::{Config, Input, Result, run_application};

/// Runs the application. See the help message for details on usage.
fn main() -> Result<()> {
    let mut input = Input::collect(env::args())?;
    let config = Config::load(input.extract_root(env::var("HOME").ok())?)?;
    run_application(input, config)
}
