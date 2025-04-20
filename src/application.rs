//! Module containing top level application functions for the binary
use std::io::{self, Write};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::input::Input;
use crate::segment_run::{
    MillisecondsSinceEpoch, SegmentRunEvent, SupplementedSegmentRun,
};

const HELP_MESSAGE: &'static str = "\
srtim is an application that tracks speedruns that support both single segments
and nested segment groups.


Global options
--------------

--root <ROOT>

By default, the config file is placed at and looked for at ${HOME}/srtim/config.tsv.
By supplying a root using this option, it is instead looked for at ${ROOT}/config.tsv


Subcommands
-----------

----
help
----
Shows this help message.

`srtim help`

---
add
---

Adds a segment or segment group to the configuration file. If `--parts` is supplied,
a segment group is being added; otherwise, a single segment is added. In either case,
<ID_NAME> is a short unique string without tabs or colons (It should be short because
it is used for the file name of the segment or group and is used to define it as part
of a segment group). <DISPLAY_NAME> is any string you'd like to show when referring to
this segment or group. It can be any unicode string without tabs.

`srtim add --id-name <ID_NAME> --display-name <DISPLAY_NAME> [--parts <PART_ID_1> <PART_ID_2> ...]`

------
delete
------

Deletes a segment or segment group from the configuration file. If the given part is
a component of a segment group in the config, then this command will error out unless
the --recursive flag is passed, and even then, the user will be asked to confirm that
they actually want to delete all of the dependencies.

`srtim delete --id-name <ID_NAME> [--recursive]`

----
tree
----

Prints the full segment specification, i.e. fully nested set of group and segment names
of a given part. This includes the full nesting.

`srtim tree --id-name <ID_NAME>`

---
run
---

Interactively runs a segment or group of segments. The app will prompt the user to hit
enter before the run begins. After that, each empty line put into stdin triggers the
end of a segment. Each non-empty line indicates a \"death\", although the application
expects such non-empty lines to contain the single letter \"d\". Otherwise, a message
asking for it to be formatted that way is printed to stderr.

`srtim run --id-name <ID_NAME>`
";

/// Adds a segment or group of segments to the config!
fn add_part_from_input(mut input: Input, mut config: Config) -> Result<()> {
    let id_name = input.extract_option_single_value("--id-name")?;
    let display_name = input.extract_option_single_value("--display-name")?;
    match input.extract_option("--parts") {
        Some((_, part_id_names)) => {
            if part_id_names.is_empty() {
                return Err(Error::EmptySegmentGroupInfo { display_name });
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

/// A trait that can be implemented by any class that can get strings
trait CustomInput {
    /// Gets a line to be processed
    fn get_line(&self) -> io::Result<String>;
}

/// A trait that can be implemented by any class
/// that can print error and non-error messages
trait CustomOutput {
    /// Prints a line of non-error output
    fn println(&self, message: String);
    /// Prints a line of error output
    fn eprintln(&self, message: String);
}

/// Implementation of the CustomInput trait to read from stdin
struct StdCustomInput {}

impl CustomInput for StdCustomInput {
    /// Reads a single line from stdin
    fn get_line(&self) -> io::Result<String> {
        let mut string = String::new();
        io::stdin().read_line(&mut string)?;
        Ok(string)
    }
}

/// Implementation of the CustomOutput trait to write to stdout/stderr
#[derive(Clone)]
struct StdCustomOutput {}

impl CustomOutput for StdCustomOutput {
    /// Prints non-error output to stdout
    fn println(&self, message: String) {
        println!("{message}");
    }

    /// Prints error output to stderr
    fn eprintln(&self, message: String) {
        eprintln!("{message}");
    }
}

/// Deletes one or more segments or groups from the config. Multiple parts
/// can be deleted only if --recursive is supplied in the input.
fn delete_part_from_input<I, O>(
    mut input: Input,
    mut config: Config,
    custom_input: I,
    custom_output: &O,
) -> Result<()>
where
    I: CustomInput,
    O: CustomOutput,
{
    let id_name = input.extract_option_single_value("--id-name")?;
    let should_save = match input.extract_option_no_values("--recursive")? {
        true => {
            let deleted = config.delete_part_recursive(&id_name)?;
            if deleted.len() > 1 {
                let mut deleted_keys: Vec<&str> = deleted
                    .keys()
                    .map(|deleted_id_name| deleted_id_name as &str)
                    .collect();
                deleted_keys.sort();
                custom_output.println(
                    format!(
                        "Recursive deletion of part with ID name \"{id_name}\" would result in deletion of {} parts: [\"{}\"]. Enter \"yes\" to confirm:",
                        deleted.len(),
                        deleted_keys.join("\", \"")
                    )
                );
                let line = custom_input
                    .get_line()
                    .map_err(|error| Error::CouldNotReadFromStdin { error })?;
                let should_save = line.trim().to_lowercase().as_str() == "yes";
                custom_output.println(match should_save {
                    true => format!(
                        "Ok! Deleting all {} of these parts.",
                        deleted.len()
                    ),
                    false => String::from("Ok! Deleting nothing."),
                });
                should_save
            } else {
                true
            }
        }
        false => {
            config.delete_part(&id_name)?;
            true
        }
    };
    if should_save {
        config.save()?;
    }
    Ok(())
}

/// Creates and writes the tree string for the
/// run specified by the --id-name in input.
fn tree_from_input<O>(
    mut input: Input,
    config: Config,
    custom_output: &O,
) -> Result<()>
where
    O: CustomOutput,
{
    let id_name = input.extract_option_single_value("--id-name")?;
    custom_output.println(config.run_tree(&id_name)?);
    Ok(())
}

/// Runs the segment using string input
fn run_part_from_input<I, O>(
    mut input: Input,
    config: Config,
    custom_input: I,
    custom_output: &O,
) -> Result<()>
where
    I: CustomInput + Send + 'static,
    O: CustomOutput + Send + Clone + 'static,
{
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
    custom_input
        .get_line()
        .map_err(|error| Error::CouldNotReadFromStdin { error })?;
    custom_output.println(String::from("Starting!"));
    let custom_output_clone = custom_output.clone();
    let input_thread = thread::spawn(move || -> Result<()> {
        let mut num_ends = 0;
        while num_ends < num_segments {
            let string = custom_input
                .get_line()
                .map_err(|error| Error::CouldNotReadFromStdin { error })?;
            let trimmed = string.trim();
            let segment_run_event = if trimmed.is_empty() {
                num_ends += 1;
                SegmentRunEvent::End
            } else {
                if trimmed != "d" {
                    custom_output_clone.eprintln(
                        String::from("Interpreting error as death even though line contained something other than just \"d\"")
                    );
                }
                SegmentRunEvent::Death
            };
            segment_run_event_sender
                .send(segment_run_event)
                .map_err(|_| Error::FailedToSendSegmentRunEvent {
                    index: (num_ends as u32) + 1,
                })?;
        }
        Ok(())
    });
    let custom_output_clone = custom_output.clone();
    let output_thread = thread::spawn(move || -> Result<()> {
        let mut start = None;
        while let Ok(SupplementedSegmentRun {
            nesting,
            name,
            segment_run,
        }) = supplemented_segment_run_receiver.recv()
        {
            if let None = start {
                start = Some(segment_run.start);
            }
            custom_output_clone.println(format!(
                "{}{name}{}, duration: {} ms, total time elapsed: {} ms",
                "\t".repeat(nesting as usize),
                if no_deaths {
                    String::new()
                } else {
                    format!(", deaths: {}", segment_run.deaths)
                },
                segment_run.duration().as_millis(),
                MillisecondsSinceEpoch::duration(
                    start.unwrap(),
                    segment_run.end,
                )?
                .as_millis(),
            ));
        }
        Ok(())
    });
    let run_result = config.run(
        &id_name,
        segment_run_event_receiver,
        supplemented_segment_run_sender,
        write,
    );
    let input_result = input_thread.join().unwrap();
    let output_result = output_thread.join().unwrap();
    input_result?;
    output_result?;
    run_result
}

/// Runs the application from input and config objects,
/// using the given custom input and output.
fn run_application_base<I, O>(
    input: Input,
    config: Config,
    custom_input: I,
    custom_output: &O,
) -> Result<()>
where
    I: CustomInput + Send + 'static,
    O: CustomOutput + Send + Clone + 'static,
{
    let mut arguments = input.arguments().iter();
    arguments.next().unwrap(); // name of the binary
    let mode = match arguments.next() {
        Some(mode) => {
            if let Some(extra_argument) = arguments.next() {
                return Err(Error::TooManyPositionalArguments {
                    extra_argument: extra_argument.clone(),
                });
            } else {
                mode as &str
            }
        }
        None => return Err(Error::NoModeFound),
    };
    match mode {
        "add" => add_part_from_input(input, config),
        "run" => {
            run_part_from_input(input, config, custom_input, custom_output)
        }
        "delete" => {
            delete_part_from_input(input, config, custom_input, custom_output)
        }
        "tree" => tree_from_input(input, config, custom_output),
        "help" => Ok(custom_output.println(String::from(HELP_MESSAGE))),
        other => {
            return Err(Error::UnknownMode {
                mode: Arc::from(other),
            });
        }
    }
}

/// Runs the application from input and config objects,
/// using stdin, stdout, and stderr for input and output.
pub fn run_application(input: Input, config: Config) -> Result<()> {
    run_application_base(input, config, StdCustomInput {}, &StdCustomOutput {})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;
    use std::ops::DerefMut;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::error::Error;
    use crate::segment_run::SegmentRun;
    use crate::utils::TempFile;
    use crate::{assert_pattern, coerce_pattern};

    #[test]
    fn test_add_part() {
        let root = temp_dir().join("test_add_part_from_input");
        let config = Config::new(root.clone());
        run_application(
            Input::collect(
                ["<>", "add", "--id-name", "a", "--display-name", "A"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap(),
            config,
        )
        .unwrap();
        let config = Config::load(root.clone()).unwrap();
        run_application(
            Input::collect(
                ["<>", "add", "--id-name", "b", "--display-name", "B"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap(),
            config,
        )
        .unwrap();
        let config = Config::load(root.clone()).unwrap();
        run_application(
            Input::collect(
                [
                    "<>",
                    "add",
                    "--id-name",
                    "abbaa",
                    "--display-name",
                    "ABBAA",
                    "--parts",
                    "a",
                    "b",
                    "b",
                    "a",
                    "a",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
            config,
        )
        .unwrap();
        let config = Config::load(root).unwrap();
        let mut nested_segment_names =
            config.nested_segment_names("abbaa").unwrap();
        assert_eq!(nested_segment_names.len(), 5);
        let mut inner_nested_segment_names =
            nested_segment_names.pop().unwrap();
        assert_eq!(inner_nested_segment_names.len(), 2);
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "A");
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "ABBAA");
        let mut inner_nested_segment_names =
            nested_segment_names.pop().unwrap();
        assert_eq!(inner_nested_segment_names.len(), 2);
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "A");
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "ABBAA");
        let mut inner_nested_segment_names =
            nested_segment_names.pop().unwrap();
        assert_eq!(inner_nested_segment_names.len(), 2);
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "B");
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "ABBAA");
        let mut inner_nested_segment_names =
            nested_segment_names.pop().unwrap();
        assert_eq!(inner_nested_segment_names.len(), 2);
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "B");
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "ABBAA");
        let mut inner_nested_segment_names =
            nested_segment_names.pop().unwrap();
        assert_eq!(inner_nested_segment_names.len(), 2);
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "A");
        assert_eq!(&inner_nested_segment_names.pop().unwrap() as &str, "ABBAA");
        let error = run_application(
            Input::collect(
                [
                    "<>",
                    "add",
                    "--id-name",
                    "c",
                    "--display-name",
                    "C",
                    "--parts",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
            config,
        );
        assert_eq!(
            &coerce_pattern!(
                error,
                Err(Error::EmptySegmentGroupInfo { display_name }),
                display_name
            ) as &str,
            "C"
        );
    }

    /// A CustomInput that simulates input from stdin by waiting a specific
    /// amount of time and then returning a specified string
    struct TestCustomInput<T: Iterator<Item = (String, u64)>> {
        iterator: Mutex<T>,
    }

    impl<T: Iterator<Item = (String, u64)>> TestCustomInput<T> {
        /// Creates a new TestCustomIO from the lines and waiting times iterator
        fn new(iterator: T) -> Self {
            Self {
                iterator: Mutex::new(iterator),
            }
        }
    }

    impl TestCustomInput<core::array::IntoIter<(String, u64), 0>> {
        /// Creates a new TestCustomInput object that does yield any lines.
        fn empty() -> Self {
            Self {
                iterator: Mutex::new(([] as [(String, u64); 0]).into_iter()),
            }
        }
    }

    impl<T: Iterator<Item = (String, u64)>> CustomInput for TestCustomInput<T> {
        /// Reaches into inner iterator and pulls out a string and number.
        /// Waits the given number of milliseconds and then returns string.
        /// Returns "Unsupported" IO error if the iterator is exhausted.
        fn get_line(&self) -> io::Result<String> {
            let mut iterator = self.iterator.lock().unwrap();
            match iterator.deref_mut().next() {
                Some((string, milliseconds_to_wait)) => {
                    thread::sleep(Duration::from_millis(milliseconds_to_wait));
                    Ok(string)
                }
                None => Err(io::ErrorKind::Unsupported.into()),
            }
        }
    }

    impl<T: Iterator<Item = (String, u64)>> Drop for TestCustomInput<T> {
        /// When drop is called on a TestCustomInput, it is ensured,
        /// at risk of panic, that all messages have been depleted.
        fn drop(&mut self) {
            if let Some(_) = self.iterator.lock().unwrap().deref_mut().next() {
                panic!("TestCustomInput's messages were not fully depleted.");
            }
        }
    }

    /// A captured output message
    #[derive(Debug, PartialEq)]
    struct TestOutputMessage {
        /// the actual string message
        message: String,
        /// True if the message is an error message, False otherwise
        is_error: bool,
    }

    /// Struct implementing CustomOutput to capture output
    #[derive(Clone)]
    struct TestCustomOutput {
        /// the captured messages
        messages: Arc<Mutex<Vec<TestOutputMessage>>>,
    }

    impl TestCustomOutput {
        /// Creates a new TestCustomOutput with empty vectors of captured messages.
        fn new() -> Self {
            Self {
                messages: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Unwraps the captured messages from the Arc<Mutex<_>> instances they are in
        fn consume(self) -> Vec<TestOutputMessage> {
            println!("{}", Arc::strong_count(&self.messages));
            Arc::try_unwrap(self.messages)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    impl CustomOutput for TestCustomOutput {
        /// Captures a new non-error message
        fn println(&self, message: String) {
            let mut messages = self.messages.lock().unwrap();
            messages.deref_mut().push(TestOutputMessage {
                message,
                is_error: false,
            });
        }

        /// Captures a new error message
        fn eprintln(&self, message: String) {
            let mut messages = self.messages.lock().unwrap();
            messages.deref_mut().push(TestOutputMessage {
                message,
                is_error: true,
            });
        }
    }

    /// Tests the run_part_from_input_base function that contains the main
    /// functionality of the run system. The only part that can't be tested
    /// is reading from stdin, so a TestLineGetter is used in place of the
    /// StdinLineGetter that is used in run_part_from_input
    #[test]
    fn test_run_part_from_input() {
        let input = Input::collect(
            ["<binary>", "run", "--id-name", "abbaa", "--include-deaths"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let root = temp_dir().join("test_run_part_from_input");
        let _temp_file_config = TempFile {
            path: root.join("config.tsv"),
        };
        let temp_file_a_segment = TempFile {
            path: root.join("a.csv"),
        };
        let temp_file_b_segment = TempFile {
            path: root.join("b.csv"),
        };
        let temp_file_segment_group = TempFile {
            path: root.join("abbaa.csv"),
        };
        let mut config = Config::new(root);
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        config.add_segment(Arc::from("b"), Arc::from("B")).unwrap();
        config
            .add_segment_group(
                Arc::from("abbaa"),
                Arc::from("ABBAA"),
                ["a", "b", "b", "a", "a"]
                    .into_iter()
                    .map(Arc::from)
                    .collect(),
            )
            .unwrap();
        config.save().unwrap();
        let custom_input = TestCustomInput::new(
            [
                ("\n", 1),
                ("d\n", 5),
                ("d\n", 5),
                ("\n", 10),
                ("d\n", 25),
                ("\n", 50),
                ("\n", 25),
                ("d\n", 10),
                ("death\n", 5),
                ("d\n", 10),
                ("\n", 5),
                ("\n", 10),
            ]
            .into_iter()
            .map(|(string, number)| (String::from(string), number)),
        );
        let custom_output = TestCustomOutput::new();
        run_application_base(input, config, custom_input, &custom_output)
            .unwrap();
        let mut custom_output = custom_output.consume().into_iter();
        let segments = SegmentRun::load_all(&temp_file_a_segment.path).unwrap();
        assert_eq!(segments.len(), 3);
        let segment = segments[0];
        assert_eq!(segment.deaths, 2);
        let first_a_duration = segment.duration();
        assert!(first_a_duration >= Duration::from_millis(19));
        let segment = segments[1];
        assert_eq!(segment.deaths, 3);
        let second_a_duration = segment.duration();
        assert!(second_a_duration >= Duration::from_millis(29));
        let segment = segments[2];
        assert_eq!(segment.deaths, 0);
        let third_a_duration = segment.duration();
        assert!(third_a_duration >= Duration::from_millis(9));
        let segments = SegmentRun::load_all(&temp_file_b_segment.path).unwrap();
        assert_eq!(segments.len(), 2);
        let segment = segments[0];
        assert_eq!(segment.deaths, 1);
        let first_b_duration = segment.duration();
        assert!(first_b_duration >= Duration::from_millis(74));
        let segment = segments[1];
        assert_eq!(segment.deaths, 0);
        let second_b_duration = segment.duration();
        assert!(second_b_duration >= Duration::from_millis(24));
        let segments =
            SegmentRun::load_all(&temp_file_segment_group.path).unwrap();
        assert_eq!(segments.len(), 1);
        let segment = segments[0];
        assert_eq!(segment.deaths, 6);
        assert!(segment.duration() >= Duration::from_millis(159));
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: String::from("Starting!"),
                is_error: false,
            }
        );
        let mut total_duration = first_a_duration;
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "\tA, deaths: 2, duration: {} ms, total time elapsed: {} ms",
                    first_a_duration.as_millis(),
                    total_duration.as_millis(),
                ),
                is_error: false,
            }
        );
        total_duration += first_b_duration;
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "\tB, deaths: 1, duration: {} ms, total time elapsed: {} ms",
                    first_b_duration.as_millis(),
                    total_duration.as_millis()
                ),
                is_error: false
            }
        );
        total_duration += second_b_duration;
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "\tB, deaths: 0, duration: {} ms, total time elapsed: {} ms",
                    second_b_duration.as_millis(),
                    total_duration.as_millis()
                ),
                is_error: false
            }
        );
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: String::from(
                    "Interpreting error as death even though line contained something other than just \"d\""
                ),
                is_error: true
            }
        );
        total_duration += second_a_duration;
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "\tA, deaths: 3, duration: {} ms, total time elapsed: {} ms",
                    second_a_duration.as_millis(),
                    total_duration.as_millis()
                ),
                is_error: false
            }
        );
        total_duration += third_a_duration;
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "\tA, deaths: 0, duration: {} ms, total time elapsed: {} ms",
                    third_a_duration.as_millis(),
                    total_duration.as_millis()
                ),
                is_error: false
            }
        );
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "ABBAA, deaths: 6, duration: {} ms, total time elapsed: {} ms",
                    total_duration.as_millis(),
                    total_duration.as_millis()
                ),
                is_error: false
            }
        );
        assert!(custom_output.next().is_none());
    }

    /// Tests the run_part_from_input_base function that contains the main
    /// functionality of the run system. Same as test_run_part_from_input,
    /// except on a single segment without the --include-deaths CLI option
    #[test]
    fn test_run_part_from_input_no_deaths() {
        let input = Input::collect(
            ["<binary>", "run", "--id-name", "a"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let root = temp_dir().join("test_run_part_from_input_no_deaths");
        let _temp_file_config = TempFile {
            path: root.join("config.tsv"),
        };
        let temp_file_a_segment = TempFile {
            path: root.join("a.csv"),
        };
        let mut config = Config::new(root);
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        config.save().unwrap();
        let custom_input = TestCustomInput::new(
            [("\n", 1), ("d\n", 5), ("d\n", 5), ("\n", 10)]
                .into_iter()
                .map(|(string, number)| (String::from(string), number)),
        );
        let custom_output = TestCustomOutput::new();
        run_application_base(input, config, custom_input, &custom_output)
            .unwrap();
        let mut custom_output = custom_output.consume().into_iter();
        let segments = SegmentRun::load_all(&temp_file_a_segment.path).unwrap();
        assert_eq!(segments.len(), 1);
        let segment = segments[0];
        assert_eq!(segment.deaths, 2);
        let duration = segment.duration();
        assert!(duration >= Duration::from_millis(19));
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: String::from("Starting!"),
                is_error: false
            }
        );
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: format!(
                    "A, duration: {} ms, total time elapsed: {} ms",
                    duration.as_millis(),
                    duration.as_millis(),
                ),
                is_error: false,
            }
        );
        assert!(custom_output.next().is_none());
    }

    /// Ensures that stdin reading errors will be caught correctly.
    fn test_run_part_from_input_error<const N: usize>(
        lines: [(&'static str, u64); N],
    ) -> Vec<TestOutputMessage> {
        let input = Input::collect(
            ["<binary>", "run", "--id-name", "a"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let root = temp_dir().join("test_run_part_from_input");
        let mut config = Config::new(root);
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        let custom_input = TestCustomInput::new(
            lines
                .into_iter()
                .map(|(string, number)| (String::from(string), number)),
        );
        let custom_output = TestCustomOutput::new();
        assert_pattern!(
            run_application_base(input, config, custom_input, &custom_output),
            Err(Error::CouldNotReadFromStdin { .. })
        );
        custom_output.consume()
    }

    /// Ensures that an error is returned if no initial input can be read.
    #[test]
    fn test_run_part_input_fail_first_stdin_read() {
        assert!(test_run_part_from_input_error([]).is_empty());
    }

    /// Ensures that an error is returned if no input can be read after initial input.
    #[test]
    fn test_run_part_input_fail_subsequent_stdin_read() {
        let mut custom_output =
            test_run_part_from_input_error([("\n", 1)]).into_iter();
        assert_eq!(
            custom_output.next().unwrap(),
            TestOutputMessage {
                message: String::from("Starting!"),
                is_error: false
            }
        );
        assert!(custom_output.next().is_none());
    }

    /// Ensures that run_application returns error if no mode is found
    #[test]
    fn test_run_application_no_mode() {
        let input =
            Input::collect(["<>"].into_iter().map(String::from)).unwrap();
        let config = Config::new(PathBuf::from("/doesnt_exist"));
        assert_pattern!(
            run_application(input, config),
            Err(Error::NoModeFound)
        );
    }

    /// Ensures that run_application returns error if no mode is found
    #[test]
    fn test_run_application_too_many_arguments() {
        let input = Input::collect(
            ["<>", "first", "second"].into_iter().map(String::from),
        )
        .unwrap();
        let config = Config::new(PathBuf::from("/doesnt_exist"));
        let extra_argument = coerce_pattern!(
            run_application(input, config),
            Err(Error::TooManyPositionalArguments { extra_argument }),
            extra_argument
        );
        assert_eq!(&extra_argument as &str, "second");
    }

    /// Ensures that run_application returns error if unknown mode is passed
    #[test]
    fn test_run_application_unknown_mode() {
        let input = Input::collect(
            ["<>", "thisisdefinitelynotamode"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let config = Config::new(PathBuf::from("/doesnt_exist"));
        let mode = coerce_pattern!(
            run_application(input, config),
            Err(Error::UnknownMode { mode }),
            mode
        );
        assert_eq!(&mode as &str, "thisisdefinitelynotamode");
    }

    /// Tests the delete subcommand when --id-name is given but with no value.
    #[test]
    fn test_delete_part_from_input_no_id_name() {
        let root = temp_dir().join("test_delete_part_from_input_no_id_name");
        let config = Config::new(root.clone());
        let input = Input::collect(
            ["<>", "delete", "--id-name"].into_iter().map(String::from),
        )
        .unwrap();
        let custom_input = TestCustomInput::empty();
        let custom_output = TestCustomOutput::new();
        let (option, values) = coerce_pattern!(
            run_application_base(input, config, custom_input, &custom_output),
            Err(Error::OptionExpectedOneValue { option, values }),
            (option, values)
        );
        assert_eq!(&option as &str, "--id-name");
        assert!(values.is_empty());
        assert!(custom_output.consume().is_empty());
    }

    /// Tests the delete subcommand where --recursive is
    /// given, but with a value instead of as a flag.
    #[test]
    fn test_delete_part_from_input_recursive_with_value() {
        let root =
            temp_dir().join("test_delete_part_from_input_recursive_with_value");
        let config = Config::new(root.clone());
        let input = Input::collect(
            [
                "<>",
                "delete",
                "--id-name",
                "a",
                "--recursive",
                "thisshouldntbehere",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let custom_input = TestCustomInput::empty();
        let custom_output = TestCustomOutput::new();
        let (option, values) = coerce_pattern!(
            run_application_base(input, config, custom_input, &custom_output),
            Err(Error::OptionExpectedNoValues { option, values }),
            (option, values)
        );
        assert_eq!(&option as &str, "--recursive");
        assert_eq!(values.len(), 1);
        assert_eq!(&values[0] as &str, "thisshouldntbehere");
    }

    /// Tests the delete subcommand without the --recursive flag, including:
    /// - Error generated when --recursive would be needed
    /// - Success when --recursive is not needed
    /// - Error generated when id name being deleted isn't present
    #[test]
    fn test_delete_part_from_input_nonrecursive() {
        let root = temp_dir().join("test_delete_part_from_input_nonrecursive");
        let _temp_file_config = TempFile {
            path: root.join("config.tsv"),
        };
        let mut config = Config::new(root.clone());
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        config
            .add_segment_group(
                Arc::from("aa"),
                Arc::from("AA"),
                Arc::from([Arc::from("a"), Arc::from("a")]),
            )
            .unwrap();
        let input = Input::collect(
            ["<>", "delete", "--id-name", "a"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        config.save().unwrap();
        assert_eq!(config.parts().len(), 2);
        {
            let custom_input = TestCustomInput::empty();
            let custom_output = TestCustomOutput::new();
            let (part_id_name, constraining_parts) = coerce_pattern!(
                run_application_base(
                    input,
                    config,
                    custom_input,
                    &custom_output
                ),
                Err(Error::CannotDeletePart {
                    part_id_name,
                    constraining_parts
                }),
                (part_id_name, constraining_parts)
            );
            assert_eq!(&part_id_name as &str, "a");
            assert_eq!(constraining_parts.len(), 1);
            assert_eq!(&constraining_parts[0] as &str, "aa");
            assert!(custom_output.consume().is_empty());
        }
        let config = Config::load(root.clone()).unwrap();
        assert_eq!(config.parts().len(), 2);
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "aa"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input = TestCustomInput::empty();
            let custom_output = TestCustomOutput::new();
            run_application_base(input, config, custom_input, &custom_output)
                .unwrap();
            assert!(custom_output.consume().is_empty());
        }
        let config = Config::load(root.clone()).unwrap();
        assert_eq!(config.parts().len(), 1);
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "a"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input = TestCustomInput::empty();
            let custom_output = TestCustomOutput::new();
            run_application_base(input, config, custom_input, &custom_output)
                .unwrap();
            assert!(custom_output.consume().is_empty());
        }
        let config = Config::load(root.clone()).unwrap();
        assert!(config.parts().is_empty());
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "a"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input = TestCustomInput::empty();
            let custom_output = TestCustomOutput::new();
            assert_eq!(
                &coerce_pattern!(
                    run_application_base(
                        input,
                        config,
                        custom_input,
                        &custom_output
                    ),
                    Err(Error::IdNameNotFound { id_name }),
                    id_name
                ) as &str,
                "a"
            );
        }
    }

    /// Tests the delete subcommand with the --recursive flag, including:
    /// - Error generated when --recursive requires user
    ///   confirmation but no input could be read
    /// - Successful no-op performed when --recursive requires
    ///   user confirmation, but user does not say yes
    /// - Successful deletion performed when --recursive requires
    ///   user confirmation and the user says yes
    /// - Successful deletion performed when --recursive doesn't require
    ///   confirmation (which is when deletion could be done without --recursive)
    /// - Error generated when id name being deleted isn't present
    #[test]
    fn test_delete_part_from_input_recursive() {
        let root = temp_dir().join("test_delete_part_from_input_recursive");
        let _temp_file_config = TempFile {
            path: root.join("config.tsv"),
        };
        let mut config = Config::new(root.clone());
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        config.add_segment(Arc::from("b"), Arc::from("B")).unwrap();
        config
            .add_segment_group(
                Arc::from("ab"),
                Arc::from("AB"),
                Arc::from([Arc::from("a"), Arc::from("b")]),
            )
            .unwrap();
        config.save().unwrap();
        assert_eq!(config.parts().len(), 3);
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "a", "--recursive"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input = TestCustomInput::empty();
            let custom_output = TestCustomOutput::new();
            let error = coerce_pattern!(
                run_application_base(
                    input,
                    config,
                    custom_input,
                    &custom_output
                ),
                Err(Error::CouldNotReadFromStdin { error }),
                error
            );
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            let custom_output = custom_output.consume();
            assert_eq!(custom_output.len(), 1);
            assert_eq!(
                coerce_pattern!(
                    &custom_output[0],
                    TestOutputMessage {
                        is_error: false,
                        message
                    },
                    message
                ) as &str,
                "Recursive deletion of part with ID name \"a\" would result in deletion of 2 parts: [\"a\", \"ab\"]. Enter \"yes\" to confirm:"
            );
        }
        let config = Config::load(root.clone()).unwrap();
        assert_eq!(config.parts().len(), 3);
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "a", "--recursive"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input =
                TestCustomInput::new([(String::from("no"), 0)].into_iter());
            let custom_output = TestCustomOutput::new();
            run_application_base(input, config, custom_input, &custom_output)
                .unwrap();
            let custom_output = custom_output.consume();
            assert_eq!(custom_output.len(), 2);
            assert_eq!(
                coerce_pattern!(
                    &custom_output[0],
                    TestOutputMessage {
                        is_error: false,
                        message
                    },
                    message
                ) as &str,
                "Recursive deletion of part with ID name \"a\" would result in deletion of 2 parts: [\"a\", \"ab\"]. Enter \"yes\" to confirm:"
            );
            assert_eq!(
                coerce_pattern!(
                    &custom_output[1],
                    TestOutputMessage {
                        is_error: false,
                        message
                    },
                    message
                ) as &str,
                "Ok! Deleting nothing."
            );
        }
        let config = Config::load(root.clone()).unwrap();
        assert_eq!(config.parts().len(), 3);
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "a", "--recursive"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input =
                TestCustomInput::new([(String::from("yEs"), 0)].into_iter());
            let custom_output = TestCustomOutput::new();
            run_application_base(input, config, custom_input, &custom_output)
                .unwrap();
            let custom_output = custom_output.consume();
            assert_eq!(custom_output.len(), 2);
            assert_eq!(
                coerce_pattern!(
                    &custom_output[0],
                    TestOutputMessage {
                        is_error: false,
                        message
                    },
                    message
                ) as &str,
                "Recursive deletion of part with ID name \"a\" would result in deletion of 2 parts: [\"a\", \"ab\"]. Enter \"yes\" to confirm:"
            );
            assert_eq!(
                coerce_pattern!(
                    &custom_output[1],
                    TestOutputMessage {
                        is_error: false,
                        message
                    },
                    message
                ) as &str,
                "Ok! Deleting all 2 of these parts."
            );
        }
        let config = Config::load(root.clone()).unwrap();
        assert_eq!(config.parts().len(), 1);
        assert!(config.parts().contains_key("b"));
        {
            let input = Input::collect(
                ["<>", "delete", "--id-name", "b", "--recursive"]
                    .into_iter()
                    .map(String::from),
            )
            .unwrap();
            let custom_input = TestCustomInput::empty();
            let custom_output = TestCustomOutput::new();
            run_application_base(input, config, custom_input, &custom_output)
                .unwrap();
        }
        let config = Config::load(root.clone()).unwrap();
        assert!(config.parts().is_empty());
    }

    /// Ensures that the tree command returns an
    /// error if no --id-name option is provided
    #[test]
    fn test_tree_from_input_no_id() {
        let config = Config::new(temp_dir().join("test_tree_from_input_no_id"));
        let input =
            Input::collect(["<>", "tree"].into_iter().map(String::from))
                .unwrap();
        let custom_output = TestCustomOutput::new();
        assert_eq!(
            &coerce_pattern!(
                run_application_base(
                    input,
                    config,
                    TestCustomInput::empty(),
                    &custom_output,
                ),
                Err(Error::OptionNotFound { option }),
                option
            ) as &str,
            "--id-name"
        );
    }

    /// Ensures that the tree command returns an error if an ID name is
    /// given that doesn't have a corresponding part in the config.
    #[test]
    fn test_tree_from_input_unknown_id() {
        let config =
            Config::new(temp_dir().join("test_tree_from_input_unknown_id"));
        let input = Input::collect(
            ["<>", "tree", "--id-name", "unknown"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let custom_output = TestCustomOutput::new();
        assert_eq!(
            &coerce_pattern!(
                run_application_base(
                    input,
                    config,
                    TestCustomInput::empty(),
                    &custom_output,
                ),
                Err(Error::IdNameNotFound { id_name }),
                id_name
            ) as &str,
            "unknown"
        );
    }

    /// Tests that the tree command prints the display name of
    /// the segment if a single segment's ID name is passed in.
    #[test]
    fn test_tree_from_input_single_segment() {
        let mut config =
            Config::new(temp_dir().join("test_tree_from_input_single_segment"));
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        let input = Input::collect(
            ["<>", "tree", "--id-name", "a"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let custom_output = TestCustomOutput::new();
        run_application_base(
            input,
            config,
            TestCustomInput::empty(),
            &custom_output,
        )
        .unwrap();
        let custom_output = custom_output.consume();
        assert_eq!(custom_output.len(), 1);
        assert_eq!(
            coerce_pattern!(
                &custom_output[0],
                TestOutputMessage {
                    is_error: false,
                    message
                },
                message
            ) as &str,
            "A"
        );
    }

    /// Tests that the tree command prints the entire nested
    /// segment tree if a group's ID name is passed in.
    #[test]
    fn test_tree_from_input_segment_group() {
        let mut config =
            Config::new(temp_dir().join("test_tree_from_input_segment_group"));
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        config
            .add_segment_group(
                Arc::from("aa"),
                Arc::from("AA"),
                Arc::from([Arc::from("a"), Arc::from("a")]),
            )
            .unwrap();
        config
            .add_segment_group(
                Arc::from("aaa"),
                Arc::from("AAA"),
                Arc::from([Arc::from("aa"), Arc::from("a")]),
            )
            .unwrap();
        let input = Input::collect(
            ["<>", "tree", "--id-name", "aaa"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let custom_output = TestCustomOutput::new();
        run_application_base(
            input,
            config,
            TestCustomInput::empty(),
            &custom_output,
        )
        .unwrap();
        let custom_output = custom_output.consume();
        assert_eq!(custom_output.len(), 1);
        assert_eq!(
            coerce_pattern!(
                &custom_output[0],
                TestOutputMessage {
                    is_error: false,
                    message
                },
                message
            ) as &str,
            "\
AAA\tAA\tA
   \t  \tA
   \tA"
        );
    }

    /// Tests that when the help subcommand is passed in. The application
    /// exits successfully after printing the pre-written help message.
    #[test]
    fn test_help_from_input() {
        let config = Config::new(temp_dir().join("test_help_from_input"));
        let input =
            Input::collect(["<>", "help"].into_iter().map(String::from))
                .unwrap();
        let custom_input = TestCustomInput::empty();
        let custom_output = TestCustomOutput::new();
        run_application_base(input, config, custom_input, &custom_output)
            .unwrap();
        let custom_output = custom_output.consume();
        assert_eq!(custom_output.len(), 1);
        assert_eq!(
            coerce_pattern!(
                &custom_output[0],
                TestOutputMessage {
                    is_error: false,
                    message
                },
                message
            ) as &str,
            HELP_MESSAGE
        );
    }
}
