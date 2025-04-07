use std::env;
use std::io::{self, Write};
use std::process;
use std::sync::mpsc;
use std::thread;

/// Adds a segment or group of segments to the config!
fn add_part(
    mut input: srtim::Input,
    mut config: srtim::Config,
) -> srtim::Result<()> {
    let id_name = input.extract_option_single_value("--id-name")?;
    let display_name = input.extract_option_single_value("--display-name")?;
    match input.extract_option("--parts") {
        Some((_, part_id_names)) => {
            if part_id_names.is_empty() {
                return Err(srtim::Error::EmptySegmentGroupInfo {
                    display_name,
                });
            } else {
                config.add_segment_group(
                    id_name,
                    display_name,
                    part_id_names,
                )?;
            }
        }
        None => {
            config.add_segment(id_name, display_name)?;
        }
    }
    config.save()?;
    Ok(())
}

/// Runs the segment using keyboard input
fn run_part(
    mut input: srtim::Input,
    config: srtim::Config,
) -> srtim::Result<()> {
    let id_name = input.extract_option_single_value("--id-name")?;
    let write = !input.extract_option_no_values("--no-write")?;
    let no_deaths = !input.extract_option_no_values("--include-deaths")?;
    let (segment_run_event_sender, segment_run_event_receiver) =
        mpsc::channel();
    let (supplemented_segment_run_sender, supplemented_segment_run_receiver) =
        mpsc::channel();
    let nested_segment_names = config.nested_segment_names(&id_name)?;
    let num_segments = nested_segment_names.len();
    print!(
        "The {} run will start when you press enter.",
        &(&nested_segment_names[0])[0]
    );
    io::stdout().flush().unwrap();
    io::stdin()
        .read_line(&mut String::new())
        .map_err(|error| srtim::Error::CouldNotReadFromStdin { error })?;
    println!("Starting!");
    let input_thread = thread::spawn(move || -> srtim::Result<()> {
        let stdin = io::stdin();
        let mut num_ends = 0;
        while num_ends < num_segments {
            let mut string = String::new();
            stdin.read_line(&mut string).map_err(|error| {
                srtim::Error::CouldNotReadFromStdin { error }
            })?;
            let trimmed = string.trim();
            let segment_run_event = if trimmed.is_empty() {
                num_ends += 1;
                srtim::SegmentRunEvent::End
            } else {
                if trimmed != "d" {
                    eprintln!("Interpreting error as death even though line contained more than just \"d\"");
                }
                srtim::SegmentRunEvent::Death
            };
            segment_run_event_sender
                .send(segment_run_event)
                .map_err(|_| srtim::Error::FailedToSendSegmentRunEvent {
                    index: (num_ends as u32) + 1,
                    event: segment_run_event,
                })?;
        }
        Ok(())
    });
    let output_thread = thread::spawn(move || -> srtim::Result<()> {
        let mut start = None;
        while let Ok(srtim::SupplementedSegmentRun {
            nesting,
            name,
            segment_run,
        }) = supplemented_segment_run_receiver.recv()
        {
            if let None = start {
                start = Some(segment_run.start);
            }
            println!(
                "{}{name}{}, duration: {} ms, total time elapsed: {} ms",
                "\t".repeat(nesting as usize),
                if no_deaths {
                    String::new()
                } else {
                    format!(", deaths: {}", segment_run.deaths)
                },
                segment_run.duration().as_millis(),
                srtim::MillisecondsSinceEpoch::duration(
                    start.unwrap(),
                    segment_run.end
                )?
                .as_millis(),
            );
        }
        Ok(())
    });
    config.run(
        &id_name,
        segment_run_event_receiver,
        supplemented_segment_run_sender,
        write,
    )?;
    input_thread.join().unwrap()?;
    output_thread.join().unwrap()?;
    Ok(())
}

fn main() -> srtim::Result<()> {
    let mut input = srtim::Input::collect(env::args())?;
    let config =
        srtim::Config::load(input.extract_root(env::var("HOME").ok())?)?;
    let mut arguments = input.arguments().iter();
    arguments.next().unwrap(); // name of the binary
    let mode = match arguments.next() {
        Some(mode) => {
            if let Some(_) = arguments.next() {
                eprintln!(
                    "the mode should be the only positional argument passed"
                );
                process::exit(1);
            } else {
                mode.as_str()
            }
        }
        None => {
            eprintln!("mode must be first argument after binary name");
            process::exit(1);
        }
    };
    match mode {
        "add" => add_part(input, config),
        "run" => run_part(input, config),
        _ => {
            eprintln!("mode should be one of [\"add\"]");
            process::exit(1);
        }
    }
}
