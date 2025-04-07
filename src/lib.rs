//! The `srtim` library represents and provides utilities to create
//! speedrun runs (with death tracking for video games).
use std::collections::HashSet;
use std::collections::{HashMap, hash_map::Entry};
use std::fmt::{self, Debug, Display};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// the component separator is used to specify parts of a group
const COMPONENT_SEPARATOR: &'static str = ":";

/// An event that can take place during a segment
#[derive(Debug, Clone, Copy)]
pub enum SegmentRunEvent {
    /// death, used in speedrunning or challenge running
    Death,
    /// end of a segment
    End,
}

impl Display for SegmentRunEvent {
    /// Display the name of the enum
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

/// Large enum used to store every possible non-panicking error in the library
pub enum Error {
    /// A duration between MillisecondsSinceEpoch objects couldn't be found
    /// because the one that is supposed to be later is actually earlier.
    NegativeDuration {
        /// the instant that is supposed to be earlier
        start: u128,
        /// the instant that is supposed to be later
        end: u128,
    },
    /// Couldn't read from stdin when expecting user input
    CouldNotReadFromStdin {
        /// IO error raised when trying to read from stdin
        error: io::Error,
    },
    /// SegmentRunFile could not be read
    CouldNotReadSegmentRunFile {
        /// location of the file being loaded
        path: PathBuf,
        /// IO error raised when trying to read the file
        error: io::Error,
    },
    /// A single line of the SegmentRun file line was invalid
    /// because it didn't have the right number of commas
    InvalidSegmentRunFileLine {
        /// index of the line that is invalid
        index: usize,
        /// true if too many elements, false if not enough
        too_many_separators: bool,
        /// the expected number of tokens per line
        expected_number_of_elements: usize,
    },
    /// a token in a SegmentRun file line couldn't
    /// be parsed as an integer like expected
    SegmentRunFileLineParseError {
        /// index of the line that is invalid
        index: usize,
        /// index of the column that is invalid
        column: u8,
    },
    /// two SegmentRun objects couldn't be combined because
    /// end of earlier doesn't coincide with start of later
    SegmentCombinationError,
    /// SegmentRun couldn't be completed because not enough events were sent
    ChannelClosedBeforeEnd,
    /// Couldn't send segment when running multiple segments in turn
    FailedToSendSegment {
        /// index of the segment that was trying to be sent
        index: u32,
    },
    /// SegmentRunEvent couldn't be sent because the receiver was closed
    FailedToSendSegmentRunEvent {
        /// index of the segment that had a failure
        index: u32,
        /// event that couldn't be sent
        event: SegmentRunEvent,
    },
    /// An error occurred during a single segment when running multiple in turn
    ErrorDuringSegment {
        /// index of the segment the error occurred during
        index: u32,
        /// error that occurred during the segment
        error: Box<Error>,
    },
    /// Segment or segment group doesn't have an absolute file
    FileNamePathsNotAbsolute {
        /// display name of the segment or segment group
        display_name: String,
    },
    /// A segment group was given that had no parts
    EmptySegmentGroupInfo {
        /// display name of the empty segment group
        display_name: String,
    },
    /// the tagging thread that turns individual segment runs into
    /// supplemented runs (including groups) couldn't proceed because
    /// no more segments were being sent
    SegmentRunSenderClosed,
    /// Couldn't save a run part
    PartSaveError {
        /// path to the file that is being written to
        path: PathBuf,
        /// the error that occurred while saving the part
        error: Box<Error>,
    },
    /// the tagging thread that turns individual segment runs into
    /// supplemented runs (including groups) couldn't proceed because
    /// the receiver it is sending supplemented segment runs was closed
    SupplementedSegmentRunReceiverClosed,
    /// the tagging thread that turns individual segment runs
    /// into supplemented runs (including groups) panicked
    SegmentTaggingThreadPanicked,
    /// an ID name was invalid because it contained disallowed character/substring(s)
    IdNameInvalid {
        /// the ID name that caused the issue
        id_name: String,
    },
    /// a display name was invalid because it contained disallowed character/substring(s)
    DisplayNameInvalid {
        /// the display name of the segment or segment group that is
        display_name: String,
    },
    /// the given ID name was not unique
    IdNameNotUnique {
        /// the ID name that already exists but was attempted to be added
        id_name: String,
    },
    /// segment group is invalid because some parts of it don't exist yet
    PartsDontExist {
        /// ID name of the segment group
        id_name: String,
        /// display name of the segment group
        display_name: String,
        /// the names of the parts that don't exist
        invalid_parts: Vec<String>,
    },
    /// a line of the config file has the wrong number of tokens
    InvalidConfigLine {
        /// index of the row that has an issue
        index: usize,
        /// true if there are too many tokens, false if there aren't enough
        too_many_separators: bool,
        /// the expected number of tokens per line
        expected_number_of_elements: usize,
    },
    /// IO error occurred while the config file was being read
    IOErrorReadingConfig(io::Error),
    /// an ID name of an existing segment or segment
    /// group was expected, but none existed
    IdNameNotFound {
        /// the ID name that doesn't correspond to an actual segment or group
        id_name: String,
    },
    /// the given run couldn't be worked with because it is too nested
    TooMuchNesting {
        /// the maximum number of nesting levels
        /// (branch nodes from root to bare segment)
        max_nesting_level: usize,
    },
    /// the config file couldn't be opened
    FailedToOpenConfigFile(io::Error),
    /// the config file couldn't be written to
    FailedToWriteToConfigFile(io::Error),
    /// a run part file couldn't be opened
    FailedToOpenRunPartFile {
        /// the path to the file that couldn't be opened
        path: PathBuf,
        /// the IO error that occurred when opening the file
        error: io::Error,
    },
    /// a run part file couldn't be written to
    FailedToWriteToRunPartFile {
        /// the path to the file that couldn't be written to
        path: PathBuf,
        /// the IO error that occured when writing to the file
        error: io::Error,
    },
    /// root wasn't given and couldn't be guessed
    /// (i.e. no HOME directory available)
    NoRootFound,
    /// the same CLI option was provided twice
    OptionProvidedTwice {
        /// the option that was provided twice
        option: String,
    },
    /// the given option had values associated with it, even
    /// though it is expected to be a flag with no values
    OptionExpectedNoValues {
        /// the option that is meant to be a flag
        option: String,
        /// all string values associated with the option
        values: Vec<String>,
    },
    /// the given option didn't have exactly one value associated with it
    OptionExpectedOneValue {
        /// the option that is meant to have one value associated with it
        option: String,
        /// the 0 or multuple values associated with the option
        values: Vec<String>,
    },
    /// the given option that was expected was not found
    OptionNotFound {
        /// the option that was expected
        option: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Debug for Error {
    /// Creates a text version of the error
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NegativeDuration { start, end } => write!(
                f,
                "negative duration from start={start} ms to end={end} ms"
            ),
            Error::CouldNotReadFromStdin { error } => {
                write!(f, "Couldn't read from stdin: {error}")
            }
            Error::CouldNotReadSegmentRunFile { path, error } => write!(
                f,
                "Couldn't read contents from {}: {error}",
                path.display()
            ),
            Error::InvalidSegmentRunFileLine {
                index,
                too_many_separators,
                expected_number_of_elements,
            } => match too_many_separators {
                true => write!(
                    f,
                    "run entry #{index} has too many commas for a segment line, which expects {} commas.",
                    expected_number_of_elements - 1,
                ),
                false => write!(
                    f,
                    "run entry #{index} does not have enough commas for a segment line, which expects {} commas.",
                    expected_number_of_elements - 1,
                ),
            },
            Error::SegmentRunFileLineParseError { index, column } => write!(
                f,
                "run entry #{index} has an unparsable integer in column with index {column}"
            ),
            Error::SegmentCombinationError => write!(
                f,
                "cannot combine two SegmentRun objects because the later one doesn't start at the end of the earlier one"
            ),
            Error::ChannelClosedBeforeEnd => write!(
                f,
                "channel was closed before all events of a single segment could be sent"
            ),
            Error::FailedToSendSegment { index } => write!(
                f,
                "segment #{index} could not be sent because the receiver was closed"
            ),
            Error::FailedToSendSegmentRunEvent { index, event } => write!(
                f,
                "segment run event #{index} ({event}) could not be sent because the receiver was closed"
            ),
            Error::ErrorDuringSegment { index, error } => write!(
                f,
                "The following error was encountered in segment #{index}: {}",
                error.as_ref()
            ),
            Error::FileNamePathsNotAbsolute { display_name } => write!(
                f,
                "file name path given to segment or segment group \"{display_name}\" not absolute"
            ),
            Error::EmptySegmentGroupInfo { display_name } => {
                write!(f, "segment group \"{display_name}\" has no components")
            }
            Error::SegmentRunSenderClosed => write!(
                f,
                "segment run sender closed before all segments were done"
            ),
            Error::PartSaveError { path, error } => write!(
                f,
                "encountered issue saving run info to file at {}: {}",
                path.display(),
                error.as_ref()
            ),
            Error::SupplementedSegmentRunReceiverClosed => write!(
                f,
                "supplemented segment run receiver closed before accepting all segments!"
            ),
            Error::SegmentTaggingThreadPanicked => {
                write!(f, "segment tagging thread panicked")
            }
            Error::DisplayNameInvalid { display_name } => write!(
                f,
                "the display name \"{display_name}\" is invalid because it contains a tab character"
            ),
            Error::IdNameInvalid { id_name } => write!(
                f,
                "the ID name \"{id_name}\" is invalid because it contains either a tab or the string used as a component separator for segment groups: \"{COMPONENT_SEPARATOR}\""
            ),
            Error::IdNameNotUnique { id_name } => write!(
                f,
                "\"{id_name}\" is the ID name of multiple segments and/or groups. ID Names must be unique!"
            ),
            Error::PartsDontExist {
                id_name,
                display_name,
                invalid_parts,
            } => write!(
                f,
                "segment group \"{id_name}\" with display name \"{display_name}\" has the following parts that are not defined before it: [{}{}{}]",
                if invalid_parts.is_empty() { "" } else { "\"" },
                invalid_parts.join("\", \""),
                if invalid_parts.is_empty() { "" } else { "\"" }
            ),
            Error::InvalidConfigLine {
                index,
                too_many_separators,
                expected_number_of_elements,
            } => match too_many_separators {
                true => write!(
                    f,
                    "config entry #{index} has too many tabs for a segment line, which expects {} tabs.",
                    expected_number_of_elements - 1,
                ),
                false => write!(
                    f,
                    "config entry #{index} does not have enough tabs for a segment line, which expects {} tabs.",
                    expected_number_of_elements - 1,
                ),
            },
            Error::IOErrorReadingConfig(io_error) => write!(
                f,
                "encountered following error reading config file: {io_error}"
            ),
            Error::IdNameNotFound { id_name } => {
                write!(f, "\"{id_name}\" is an unknown ID name")
            }
            Error::TooMuchNesting { max_nesting_level } => write!(
                f,
                "cannot handle more than {max_nesting_level} levels of nesting"
            ),
            Error::FailedToOpenConfigFile(io_error) => write!(
                f,
                "encountered following error opening config file to save: {io_error}"
            ),
            Error::FailedToWriteToConfigFile(io_error) => write!(
                f,
                "encountered following error writing to config file: {io_error}"
            ),
            Error::FailedToOpenRunPartFile { path, error } => write!(
                f,
                "encountered following error opening run part file at {} to save: {error}",
                path.display()
            ),
            Error::FailedToWriteToRunPartFile { path, error } => write!(
                f,
                "encountered following error writing to run part file at {}: {error}",
                path.display()
            ),
            Error::NoRootFound => write!(
                f,
                "--root wasn't supplied, but no HOME directory is known"
            ),
            Error::OptionProvidedTwice { option } => {
                write!(f, "option \"{option}\" was provided twice")
            }
            Error::OptionExpectedNoValues { option, values } => write!(
                f,
                "option \"{option}\" expected no values, but got {}: [{}{}{}]",
                values.len(),
                if values.is_empty() { "" } else { "\"" },
                values.join("\", \""),
                if values.is_empty() { "" } else { "\"" }
            ),
            Error::OptionExpectedOneValue { option, values } => write!(
                f,
                "option \"{option}\" expected one value, but got {}: [{}{}{}]",
                values.len(),
                if values.is_empty() { "" } else { "\"" },
                values.join("\", \""),
                if values.is_empty() { "" } else { "\"" }
            ),
            Error::OptionNotFound { option } => {
                write!(f, "option \"{option}\" not found")
            }
        }
    }
}

impl Display for Error {
    /// Display::fmt is same as Debug::fmt for Error
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

/// A struct to store a specific instant in time via the number of milliseconds since the unix epoch
#[derive(Clone, Copy)]
pub struct MillisecondsSinceEpoch(pub u128);
impl MillisecondsSinceEpoch {
    pub fn now() -> MillisecondsSinceEpoch {
        MillisecondsSinceEpoch(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
    }

    /// finds the amount of time between the two times.
    pub fn duration(
        start: MillisecondsSinceEpoch,
        end: MillisecondsSinceEpoch,
    ) -> Result<Duration> {
        if end.0 >= start.0 {
            Ok(Duration::from_millis(
                (end.0 - start.0)
                    .try_into()
                    .expect("program can only handle durations of 2^64 ms"),
            ))
        } else {
            Err(Error::NegativeDuration {
                start: start.0,
                end: end.0,
            })
        }
    }
}

/// Loads n string slices from the given iterator, returning an error
/// containing whether there were too many or too few tokens available
fn load_n_tokens<'a>(
    tokens: impl Iterator<Item = &'a str>,
    expected_number: usize,
) -> std::result::Result<Vec<&'a str>, bool> {
    let result: Vec<&'a str> = tokens.take(expected_number + 1).collect();
    let length = result.len();
    if length == expected_number {
        Ok(result)
    } else {
        Err(length > expected_number)
    }
}

#[derive(Clone, Copy)]
/// Information about a single segment run
pub struct SegmentRun {
    /// the number of deaths in the run
    pub deaths: u32,
    /// the instant the run started
    pub start: MillisecondsSinceEpoch,
    /// the instant the run ended
    pub end: MillisecondsSinceEpoch,
}

impl SegmentRun {
    fn accumulate(&mut self, next: &Self) -> Result<()> {
        if self.end.0 == next.start.0 {
            self.deaths += next.deaths;
            self.end = next.end;
            Ok(())
        } else {
            Err(Error::SegmentCombinationError)
        }
    }

    fn accumulate_or_start(
        maybe_started: Option<Self>,
        next: &Self,
    ) -> Result<Self> {
        match maybe_started {
            Some(mut started) => {
                started.accumulate(next)?;
                Ok(started)
            }
            None => Ok(next.clone()),
        }
    }

    /// Runs a single segment, assuming a given start time. The
    /// borrowed receiver will listen for events during the run.
    pub fn run(
        receiver: &Receiver<SegmentRunEvent>,
        start: MillisecondsSinceEpoch,
    ) -> Result<SegmentRun> {
        let mut deaths: u32 = 0u32;
        loop {
            match receiver.recv() {
                Ok(SegmentRunEvent::End) => {
                    break;
                }
                Ok(SegmentRunEvent::Death) => {
                    deaths += 1;
                }
                Err(_) => {
                    return Err(Error::ChannelClosedBeforeEnd);
                }
            };
        }
        let end = MillisecondsSinceEpoch::now();
        Ok(SegmentRun { deaths, start, end })
    }

    /// The amount of time a run took as a Duration object
    pub fn duration(&self) -> Duration {
        Duration::from_millis((self.end.0 - self.start.0).try_into().expect(
            "The program cannot handle durations of 2^64 or more milliseconds",
        ))
    }

    /// Runs the given number of segments in a row, starting inside this function.
    /// The given receiver listens for events like the ends of segments and deaths.
    /// The given sender will be used to send out individual segments as they are completed.
    pub fn run_consecutive(
        num_segments: u32,
        segment_run_event_receiver: Receiver<SegmentRunEvent>,
        segment_run_sender: Sender<SegmentRun>,
    ) -> Result<()> {
        let mut start = MillisecondsSinceEpoch::now();
        for index in 1..(num_segments + 1) {
            match SegmentRun::run(&segment_run_event_receiver, start) {
                Ok(segment_run) => {
                    start = segment_run.end;
                    if let Err(_) = segment_run_sender.send(segment_run) {
                        return Err(Error::FailedToSendSegment { index });
                    }
                }
                Err(error) => {
                    return Err(Error::ErrorDuringSegment {
                        index,
                        error: Box::new(error),
                    });
                }
            }
        }
        Ok(())
    }

    /// Adds a new line to the runs of the part that has the
    /// given path. Creates file if it doesn't already exist.
    fn save(&self, path: &Path) -> Result<()> {
        let io_error_map = |error: io::Error| -> Error {
            Error::FailedToOpenRunPartFile {
                path: PathBuf::from(path),
                error,
            }
        };
        let mut file =
            match File::options().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write(b"start,end,deaths\n").map_err(io_error_map)?;
                    Ok(file)
                }
                Err(error) => {
                    if error.kind() == io::ErrorKind::AlreadyExists {
                        Ok(File::options()
                            .write(true)
                            .append(true)
                            .open(&path)
                            .map_err(io_error_map)?)
                    } else {
                        Err(io_error_map(error))
                    }
                }
            }?;
        file.write(
            format!("{},{},{}\n", self.start.0, self.end.0, self.deaths)
                .as_bytes(),
        )
        .map_err(io_error_map)?;
        Ok(())
    }

    /// Loads all SegmentRun objects stored in a csv file.
    pub fn load_all(path: &Path) -> Result<Vec<SegmentRun>> {
        let contents = fs::read_to_string(path).map_err(|error| {
            Error::CouldNotReadSegmentRunFile {
                path: path.to_path_buf(),
                error,
            }
        })?;
        let mut lines = contents
            .lines()
            .map(str::trim)
            .filter(|&s| !s.is_empty())
            .enumerate();
        lines.next();
        let mut result: Vec<SegmentRun> = Vec::new();
        let expected_number_of_elements = 3 as usize;
        for (index, line) in lines {
            let tokens =
                load_n_tokens(line.split(','), expected_number_of_elements)
                    .map_err(|too_many_separators| {
                        Error::InvalidSegmentRunFileLine {
                            index,
                            too_many_separators,
                            expected_number_of_elements,
                        }
                    })?;
            let start =
                MillisecondsSinceEpoch(tokens[0].parse().map_err(|_| {
                    Error::SegmentRunFileLineParseError { index, column: 0 }
                })?);
            let end =
                MillisecondsSinceEpoch(tokens[1].parse().map_err(|_| {
                    Error::SegmentRunFileLineParseError { index, column: 1 }
                })?);
            let deaths: u32 = tokens[2].parse().map_err(|_| {
                Error::SegmentRunFileLineParseError { index, column: 2 }
            })?;
            result.push(SegmentRun { start, end, deaths });
        }
        Ok(result)
    }
}

/// Logistical information about a single segment, including the name that
/// should be displayed and the file in which its information is stored.
struct SegmentInfo {
    /// name that should be displayed for this segment
    display_name: String,
    /// absolute path to file containing this segment's info
    file_name: PathBuf,
}

fn check_display_name(display_name: &str) -> Result<()> {
    if display_name.contains('\t') {
        Err(Error::DisplayNameInvalid {
            display_name: String::from(display_name),
        })
    } else {
        Ok(())
    }
}

impl SegmentInfo {
    /// Creates a new SegmentInfo, ensuring that file_name is an absolute path
    pub fn new(
        display_name: String,
        file_name: PathBuf,
    ) -> Result<SegmentInfo> {
        check_display_name(&display_name)?;
        if file_name.is_absolute() {
            Ok(SegmentInfo {
                display_name,
                file_name,
            })
        } else {
            Err(Error::FileNamePathsNotAbsolute { display_name })
        }
    }
}

/// Struct to store the owned specification of a segment group
struct SegmentGroupInfo {
    /// name that should be displayed for this group
    display_name: String,
    /// non-empty list of ID names of the parts that make up this group
    part_id_names: Vec<String>,
    /// absolute path to file containing information about this group's runs
    file_name: PathBuf,
}

impl SegmentGroupInfo {
    /// Creates a new SegmentGroupInfo, ensuring file_name is an absolute path and part_id_names is non-empty
    fn new(
        display_name: String,
        part_id_names: Vec<String>,
        file_name: PathBuf,
    ) -> Result<SegmentGroupInfo> {
        check_display_name(&display_name)?;
        if file_name.is_relative() {
            Err(Error::FileNamePathsNotAbsolute { display_name })
        } else if part_id_names.is_empty() {
            Err(Error::EmptySegmentGroupInfo { display_name })
        } else {
            Ok(SegmentGroupInfo {
                display_name,
                part_id_names,
                file_name,
            })
        }
    }
}

/// A single part of a run, which could be a single segment or a group of segments or other groups
enum RunPart<'a> {
    /// part of a run that is just a single segment
    SingleSegment(&'a SegmentInfo),
    /// part of a run that is a (possibly nested) group of segments
    Group {
        components: Vec<&'a RunPart<'a>>,
        display_name: &'a str,
        file_name: &'a Path,
    },
}

/// Gets the segment specifications for each low-level segment in the run.
/// Each segment spec is a vector whose elements are (index, part) where
/// index is the index into the previous part vector and part is the next level.
///
/// For example, if compoents represents (((AB)C)D), then the elements would be
/// [(0, ((AB)C)), (0, (AB)), (0, A)],
/// [(0, ((AB)C)), (0, (AB)), (1, B)],
/// [(0, ((AB)C)), (1, C)],
/// [(1, D)]
fn nested_segment_specs_from_parts_vec<'a>(
    components: &'a Vec<&'a RunPart<'a>>,
) -> Vec<Vec<(usize, &'a RunPart<'a>)>> {
    let mut result: Vec<Vec<(usize, &'a RunPart<'a>)>> = Vec::new();
    for (index, component) in components.iter().enumerate() {
        match component {
            RunPart::Group { components, .. } => {
                let mut leaves =
                    nested_segment_specs_from_parts_vec(components).into_iter();
                while let Some(leaf) = leaves.next() {
                    let mut full_segment_spec = vec![(index, *component)];
                    let mut nodes = leaf.into_iter();
                    while let Some(node) = nodes.next() {
                        full_segment_spec.push(node);
                    }
                    result.push(full_segment_spec);
                }
            }
            _ => {
                result.push(vec![(index, *component)]);
            }
        };
    }
    result
}

/// Description of a segment or group that was just completed
pub struct SupplementedSegmentRun {
    /// nesting level of the part that was just completed (how many groups into the run this is)
    pub nesting: u8,
    /// the name of the run part (either segment or group) that was completed
    pub name: String,
    /// number of deaths and start and end times of the part that was just completed
    pub segment_run: SegmentRun,
}

/// Struct that iterates through two separate iterators to find all the elements on the front that are equal
struct ZipSame<T: PartialEq, U: Iterator<Item = T>, V: Iterator<Item = T>> {
    /// two iterators to check how long they are equal
    iterators: (U, V),
    /// whether or not the ZipSame object has already found a difference
    /// between the iterators (or the end of either of them)
    depleted: bool,
}

/// ZipSame implements the same Iterator trait as U (source iterator
/// type) because it returns elements from the source iterators.
impl<T: PartialEq, U: Iterator<Item = T>, V: Iterator<Item = T>> Iterator
    for ZipSame<T, U, V>
{
    type Item = T;

    /// If no difference has been found yet, advances both iterators and returns the first's
    /// value if they are equal. Otherwise, return None and never return Some again
    fn next(&mut self) -> Option<Self::Item> {
        if self.depleted {
            None
        } else {
            let first = self.iterators.0.next();
            let second = self.iterators.1.next();
            match first {
                Some(first) => match second {
                    Some(second) => {
                        if first == second {
                            Some(first)
                        } else {
                            self.depleted = true;
                            None
                        }
                    }
                    None => {
                        self.depleted = true;
                        None
                    }
                },
                None => {
                    self.depleted = true;
                    None
                }
            }
        }
    }
}

/// Creates an iterator that will iterate through first and second (which iterate over
/// the same type) and yield elements that are equal until one of the iterators is exhausted
/// or the iterator's are advanced to the point that they yield different elements.
fn zip_same<T: PartialEq, U: Iterator<Item = T>, V: Iterator<Item = T>>(
    first: U,
    second: V,
) -> impl Iterator<Item = T> {
    ZipSame {
        iterators: (first, second),
        depleted: false,
    }
}

/// Owned display and path names for a given run part
struct PartCore {
    /// the human-readable name of the part
    display_name: String,
    /// the path where the part's runs are stored
    file_name: PathBuf,
}

/// PartialEq needs to be implemented for PartCore for it to be used in ZipSame
impl PartialEq for PartCore {
    /// Since display names can be duplicated but paths cannot, == is just checks paths
    fn eq(&self, other: &Self) -> bool {
        self.file_name == other.file_name
    }
}

impl<'a> RunPart<'a> {
    /// Gets the display name of the part, whether it is a segment or a segment group
    fn display_name(&'a self) -> &'a str {
        match self {
            &RunPart::Group { display_name, .. } => display_name,
            &RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                display_name.as_str()
            }
        }
    }

    /// Gets the file name of the part, whether it is a segment or a segment group
    fn file_name(&'a self) -> &'a Path {
        match self {
            &RunPart::Group { file_name, .. } => file_name,
            &RunPart::SingleSegment(SegmentInfo { file_name, .. }) => {
                file_name.as_path()
            }
        }
    }

    /// Gets the specs for all low-level segments of the run (i.e. how they connect back to the full run)
    fn nested_segment_specs(&'a self) -> Vec<Vec<(usize, &'a RunPart<'a>)>> {
        match self {
            RunPart::Group { components, .. } => {
                nested_segment_specs_from_parts_vec(components)
            }
            _ => vec![],
        }
    }

    /// Gets the nested segment names (the single top-level name is always implied)
    /// when the run part is a group, None if the run part is a single segment.
    ///
    /// For example, if the full run part is (((AB)C)D) then the vectors will contain:
    /// [[((AB)C), (AB), A], [((AB)C), (AB), B], [((AB)C), C], [D]]
    fn nested_segment_names(&self) -> Vec<Vec<PartCore>> {
        self.nested_segment_specs()
            .into_iter()
            .map(|segment_spec| {
                segment_spec
                    .into_iter()
                    .map(|(_, part)| PartCore {
                        display_name: String::from(part.display_name()),
                        file_name: PathBuf::from(part.file_name()),
                    })
                    .collect()
            })
            .collect()
    }

    /// Starts a timed run using this RunPart as the root of the run tree
    ///
    /// Args:
    ///     segment_run_event_receiver: receiving side of a channel where
    ///         events like deaths and segment completions are sent. In order
    ///         to run successfully, the sender must send the correct number of
    ///         segment ends
    ///     supplemented_segment_run_sender: sending side of a channel where
    ///         completed run parts will be sent as they are completed. Every
    ///         run
    fn run(
        &'a self,
        segment_run_event_receiver: Receiver<SegmentRunEvent>,
        supplemented_segment_run_sender: Sender<SupplementedSegmentRun>,
        write: bool,
    ) -> Result<()> {
        let write_function =
            |segment_run: SegmentRun, path: &Path| -> Result<()> {
                segment_run
                    .save(path)
                    .map_err(|error| Error::PartSaveError {
                        path: PathBuf::from(path),
                        error: Box::new(error),
                    })
            };
        let (segment_run_sender, segment_run_receiver) = mpsc::channel();
        let nested_segment_names = self.nested_segment_names();
        let num_segments = match nested_segment_names.len() {
            0 => 1usize,
            num_segments => num_segments,
        };
        let top_level_name = String::from(self.display_name());
        let top_level_path = PathBuf::from(self.file_name());
        let mut maybe_top_level_accumulation: Option<SegmentRun> = None;
        let tagging_thread = thread::spawn(move || {
            let mut nested_segment_names = nested_segment_names.into_iter();
            let mut last_segment_names = match nested_segment_names.next() {
                Some(last_segment_names) => last_segment_names,
                None => vec![],
            };
            let mut accumulations: HashMap<String, SegmentRun> = HashMap::new();
            loop {
                let next_segment_names = match nested_segment_names.next() {
                    Some(next_segment_names) => next_segment_names,
                    None => vec![],
                };
                let mut nesting = last_segment_names.len() as u8;
                let segment_run = segment_run_receiver
                    .recv()
                    .map_err(|_| Error::SegmentRunSenderClosed)?;
                maybe_top_level_accumulation =
                    Some(SegmentRun::accumulate_or_start(
                        maybe_top_level_accumulation,
                        &segment_run,
                    )?);
                let mut num_continuing_parts = 0u8;
                for PartCore {
                    display_name: continuing_part_name,
                    ..
                } in zip_same(
                    last_segment_names.iter(),
                    next_segment_names.iter(),
                ) {
                    num_continuing_parts += 1;
                    match accumulations.entry(continuing_part_name.clone()) {
                        Entry::Occupied(mut entry) => {
                            entry.get_mut().accumulate(&segment_run)?;
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(segment_run);
                        }
                    }
                }
                let num_continuing_parts = num_continuing_parts as usize;
                while last_segment_names.len() > num_continuing_parts {
                    let PartCore {
                        display_name: last_segment_name,
                        file_name: last_segment_path,
                    } = last_segment_names.pop().unwrap();
                    let full_segment_run =
                        match accumulations.remove_entry(&last_segment_name) {
                            Some((_, mut full_segment_run)) => {
                                full_segment_run.accumulate(&segment_run)?;
                                full_segment_run
                            }
                            None => segment_run,
                        };
                    match supplemented_segment_run_sender.send(
                        SupplementedSegmentRun {
                            nesting,
                            name: last_segment_name,
                            segment_run: full_segment_run,
                        },
                    ) {
                        Ok(()) => {
                            if write {
                                write_function(
                                    full_segment_run,
                                    &last_segment_path,
                                )?;
                            }
                        }
                        Err(_) => {
                            return Err(
                                Error::SupplementedSegmentRunReceiverClosed,
                            );
                        }
                    }
                    nesting -= 1;
                }
                last_segment_names = next_segment_names;
                if last_segment_names.is_empty() {
                    break;
                }
            }
            let top_level_segment_run = maybe_top_level_accumulation.unwrap();
            match supplemented_segment_run_sender.send(SupplementedSegmentRun {
                nesting: 0,
                name: top_level_name,
                segment_run: top_level_segment_run,
            }) {
                Ok(()) => {
                    if write {
                        write_function(top_level_segment_run, &top_level_path)?;
                    }
                    Ok(())
                }
                Err(_) => Err(Error::SupplementedSegmentRunReceiverClosed),
            }
        });
        SegmentRun::run_consecutive(
            num_segments as u32,
            segment_run_event_receiver,
            segment_run_sender,
        )?;
        match tagging_thread.join() {
            Ok(result) => result,
            Err(_) => Err(Error::SegmentTaggingThreadPanicked),
        }
    }
}

/// Represents a single entry of the config file
enum ConfigPart {
    /// an entry of the config file corresponding to an indivisible segment
    Segment(SegmentInfo),
    /// an entry of the config file corresponding to a group of segments
    Group(SegmentGroupInfo),
}

impl ConfigPart {
    /// Gets the display name of the part
    fn display_name(&self) -> &String {
        match self {
            ConfigPart::Segment(segment_info) => &segment_info.display_name,
            ConfigPart::Group(segment_group_info) => {
                &segment_group_info.display_name
            }
        }
    }
}

/// top-level config that works as a pool for all segments and segment groups
pub struct Config {
    root: PathBuf,
    parts: HashMap<String, ConfigPart>,
}

impl Config {
    /// the stem of the config path
    const FILE_NAME: &'static str = "config.tsv";

    /// creates a new empty config with the given root directory
    pub fn new(root: PathBuf) -> Config {
        Config {
            root,
            parts: HashMap::new(),
        }
    }

    /// finds the config file inside the given root directory
    pub fn file_name(root: &Path) -> PathBuf {
        root.join(PathBuf::from(Self::FILE_NAME))
    }

    /// Adds a part (either a segment or a group of segments) with the given ID name.
    /// An error can be returned if
    /// 1. the ID name is already taken
    /// 2. components of a segment group are not yet defined
    /// 3. the ID name has the special component separator (':'), which is not allowed
    fn add_part(&mut self, id_name: String, part: ConfigPart) -> Result<()> {
        if id_name.contains(COMPONENT_SEPARATOR) || id_name.contains('\t') {
            return Err(Error::IdNameInvalid { id_name });
        }
        if let ConfigPart::Group(SegmentGroupInfo { part_id_names, .. }) = &part
        {
            if part_id_names.iter().any(|s| !self.parts.contains_key(s)) {
                return Err(Error::PartsDontExist {
                    id_name,
                    display_name: part.display_name().clone(),
                    invalid_parts: part_id_names
                        .iter()
                        .filter(|s| !self.parts.contains_key(*s))
                        .map(|s| s.clone())
                        .collect(),
                });
            }
        }
        match self.parts.entry(id_name) {
            Entry::Occupied(e) => Err(Error::IdNameNotUnique {
                id_name: e.key().clone(),
            }),
            Entry::Vacant(e) => {
                e.insert(part);
                Ok(())
            }
        }
    }

    /// Adds info about a segment to the config
    pub fn add_segment(
        &mut self,
        id_name: String,
        display_name: String,
    ) -> Result<()> {
        let file_name = self.root.join(format!("{}.csv", id_name));
        let segment = SegmentInfo::new(display_name, file_name)?;
        self.add_part(id_name, ConfigPart::Segment(segment))
    }

    /// Adds info about a segment group to the config
    pub fn add_segment_group(
        &mut self,
        id_name: String,
        display_name: String,
        part_id_names: Vec<String>,
    ) -> Result<()> {
        let file_name = self.root.join(format!("{}.csv", id_name));
        let segment_group =
            SegmentGroupInfo::new(display_name, part_id_names, file_name)?;
        self.add_part(id_name, ConfigPart::Group(segment_group))
    }

    /// Loads a single line of the from the config file. index is the index of the line
    /// in the file, which is the index starting at 1 of the entry in the file.
    fn load_part_line(&mut self, index: usize, line: &str) -> Result<()> {
        let expected_number_of_elements: usize = 3;
        let elements = load_n_tokens(
            line.split('\t').map(str::trim),
            expected_number_of_elements,
        )
        .map_err(|too_many_separators| Error::InvalidConfigLine {
            index,
            too_many_separators,
            expected_number_of_elements,
        })?;
        let mut iter = elements.into_iter();
        let id_name = String::from(iter.next().unwrap());
        let display_name = String::from(iter.next().unwrap());
        let part_id_names: Vec<String> = iter
            .next()
            .unwrap()
            .split(COMPONENT_SEPARATOR)
            .filter(|s| !s.is_empty())
            .map(|p| String::from(p))
            .collect();
        match part_id_names.len() {
            0 => self.add_segment(id_name, display_name),
            _ => self.add_segment_group(id_name, display_name, part_id_names),
        }
    }

    /// Loads a config from a file whose lines are of the form "id_name,display_name,component_id_names"
    pub fn load(root: PathBuf) -> Result<Config> {
        let mut config = Self::new(root);
        let contents = match fs::read_to_string(Self::file_name(&config.root)) {
            Ok(contents) => contents,
            Err(error) => {
                return match error.kind() {
                    io::ErrorKind::NotFound => Ok(config),
                    _ => Err(Error::IOErrorReadingConfig(error)),
                };
            }
        };
        let mut lines = contents.lines().enumerate();
        lines.next();
        for (index, line) in lines {
            config.load_part_line(index, line)?;
        }
        Ok(config)
    }

    /// Saves the config to the config file corresponding to this Config object's root directory
    pub fn save(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .map_err(Error::FailedToOpenConfigFile)?;
        let mut file = File::create(Self::file_name(&self.root))
            .map_err(Error::FailedToOpenConfigFile)?;
        file.write("id_name\tdisplay_name\tcomponent_id_names\n".as_bytes())
            .map_err(Error::FailedToWriteToConfigFile)?;
        let mut queue: Vec<(&str, &ConfigPart)> = Vec::new();
        queue.reserve(self.parts.len());
        let mut in_queue: HashSet<&str> = HashSet::new();
        in_queue.reserve(self.parts.len());
        while queue.len() < self.parts.len() {
            for (id_name, part_info) in &self.parts {
                let id_name = id_name.as_str();
                if !in_queue.contains(id_name) {
                    let should_add = match part_info {
                        ConfigPart::Segment(_) => true,
                        ConfigPart::Group(SegmentGroupInfo {
                            part_id_names,
                            ..
                        }) => part_id_names.iter().all(|part_id_name| {
                            in_queue.contains(part_id_name.as_str())
                        }),
                    };
                    if should_add {
                        queue.push((id_name, part_info));
                        in_queue.insert(id_name);
                    }
                }
            }
        }
        drop(in_queue);
        for (id_name, part_info) in queue {
            let line = match part_info {
                ConfigPart::Segment(SegmentInfo { display_name, .. }) => {
                    format!("{id_name}\t{display_name}\t\n")
                }
                ConfigPart::Group(SegmentGroupInfo {
                    display_name,
                    part_id_names,
                    ..
                }) => format!(
                    "{id_name}\t{display_name}\t{}\n",
                    part_id_names.join(COMPONENT_SEPARATOR)
                ),
            };
            file.write(line.as_bytes())
                .map_err(Error::FailedToWriteToConfigFile)?;
        }
        Ok(())
    }
}

/// A struct that stores run parts at each level of nesting:
/// - first_level is set of parts that are raw segments
/// - second level contains at least one group of segments
/// - third level contains at least one group from second level
/// - etc.
struct SegmentPool<'a, 'b, 'c, 'd, 'e>
where
    'a: 'b,
    'b: 'c,
    'c: 'd,
    'd: 'e,
{
    first_level: Vec<RunPart<'a>>,
    second_level: Vec<RunPart<'b>>,
    third_level: Vec<RunPart<'c>>,
    fourth_level: Vec<RunPart<'d>>,
    fifth_level: Vec<RunPart<'e>>,
}

impl<'a, 'b, 'c, 'd, 'e> SegmentPool<'a, 'b, 'c, 'd, 'e>
where
    'a: 'b,
    'b: 'c,
    'c: 'd,
    'd: 'e,
{
    /// number of group levels that can exist (note that this is one less than number of levels)
    const MAX_NESTING_LEVEL: usize = 4;

    /// Create a new empty set of levels
    fn new() -> SegmentPool<'a, 'b, 'c, 'd, 'e> {
        SegmentPool {
            first_level: Vec::new(),
            second_level: Vec::new(),
            third_level: Vec::new(),
            fourth_level: Vec::new(),
            fifth_level: Vec::new(),
        }
    }

    /// Finds the full nested information of the run.
    fn run_info<'z>(&'a self) -> &'z RunPart<'z>
    where
        'e: 'z,
    {
        for level in [
            &self.fifth_level,
            &self.fourth_level,
            &self.third_level,
            &self.second_level,
        ] {
            if let Some(top_level) = level.last() {
                return top_level;
            }
        }
        self.first_level.last().unwrap()
    }

    /// Makes a level of the SegmentPool from named ConfigParts, their orders, and the previous levels
    fn make_level<'y, 'z, const N: usize>(
        parts: &'y HashMap<String, ConfigPart>,
        order: &'z HashMap<&'y str, usize>,
        existing: [(&'z HashMap<&'y str, usize>, &'z Vec<RunPart<'z>>); N],
    ) -> (HashMap<&'y str, usize>, Vec<RunPart<'z>>)
    where
        'y: 'z,
    {
        let mut new = Vec::new();
        let mut indices = HashMap::new();
        order
            .iter()
            .filter(|(_, this_order)| **this_order == N)
            .map(|(&name, _)| name)
            .enumerate()
            .for_each(|(index, id_name)| {
                match N {
                    0 => match parts.get(id_name).unwrap() {
                        ConfigPart::Segment(segment_info) => {
                            new.push(RunPart::SingleSegment(segment_info));
                        }
                        _ => panic!("This shouldn't ever happen!"),
                    },
                    _ => match parts.get(id_name).unwrap() {
                        ConfigPart::Group(SegmentGroupInfo {
                            display_name,
                            part_id_names,
                            file_name,
                        }) => {
                            let mut components = Vec::new();
                            components.reserve(part_id_names.len());
                            for part_id_name in part_id_names {
                                let part_id_name = part_id_name.as_str();
                                let this_order = order[part_id_name];
                                let (existing_indices, existing_level) =
                                    existing[this_order];
                                components.push(
                                    &existing_level
                                        [existing_indices[part_id_name]],
                                );
                            }
                            new.push(RunPart::Group {
                                display_name: display_name.as_str(),
                                components,
                                file_name: file_name.as_path(),
                            });
                        }
                        _ => panic!("This shouldn't ever happen!"),
                    },
                }
                indices.insert(id_name, index);
            });
        (indices, new)
    }
}

impl<'a> Config {
    /// Finds the order of all ConfigParts relevant to the given ID name. For example:
    /// - if the ID name is a segment, returns a hash map containing only (id_name, 0)
    /// - if the ID name is a group, then returns a hash map whose keys are the names
    ///   of all relevant parts and whose values are such that each segment has a value
    ///   of 0 and each group has a value 1 greater than the values associated with all
    ///   its components
    ///
    /// The returned map is guaranteed to be non-empty if it exists. None is only returned
    /// if there is no group or segment by the given name.
    fn order(&'a self, id_name: &'a str) -> Option<HashMap<&'a str, usize>> {
        let mut result: HashMap<&'a str, usize> = HashMap::new();
        let mut stack: Vec<&'a str> = vec![id_name];
        while let Some(current) = stack.pop() {
            if result.contains_key(current) {
                continue;
            }
            match self.parts.get(current)? {
                ConfigPart::Segment(_) => {
                    result.insert(current, 0);
                }
                ConfigPart::Group(SegmentGroupInfo {
                    part_id_names, ..
                }) => {
                    let mut order: usize = 0;
                    let mut to_push: Vec<&'a str> = Vec::new();
                    for part_id_name in part_id_names {
                        let part_id_name = part_id_name.as_str();
                        match result.get(part_id_name) {
                            Some(&sub_order) => {
                                let minimum_order = 1 + sub_order;
                                if minimum_order > order {
                                    order = minimum_order;
                                }
                            }
                            None => {
                                to_push.push(part_id_name);
                            }
                        }
                    }
                    if to_push.is_empty() {
                        result.insert(current, order);
                    } else {
                        stack.push(current);
                        to_push.into_iter().for_each(|s| stack.push(s));
                    }
                }
            }
        }
        Some(result)
    }

    /// Constructs a run tree and runs the given function on it, returning the result of the function.
    fn use_run_info<F, R>(&'a self, id_name: &'a str, f: F) -> Result<R>
    where
        F: FnOnce(&RunPart) -> R,
    {
        let order =
            self.order(id_name).ok_or_else(|| Error::IdNameNotFound {
                id_name: String::from(id_name),
            })?;
        if order.iter().map(|(_, &order)| order).max().unwrap()
            > SegmentPool::MAX_NESTING_LEVEL
        {
            return Err(Error::TooMuchNesting {
                max_nesting_level: SegmentPool::MAX_NESTING_LEVEL,
            });
        }
        let mut pool = SegmentPool::new();
        let (first_level_indices, first_level) =
            SegmentPool::make_level(&self.parts, &order, []);
        pool.first_level = first_level;
        let (second_level_indices, second_level) = SegmentPool::make_level(
            &self.parts,
            &order,
            [(&first_level_indices, &pool.first_level)],
        );
        pool.second_level = second_level;
        let (third_level_indices, third_level) = SegmentPool::make_level(
            &self.parts,
            &order,
            [
                (&first_level_indices, &pool.first_level),
                (&second_level_indices, &pool.second_level),
            ],
        );
        pool.third_level = third_level;
        let (fourth_level_indices, fourth_level) = SegmentPool::make_level(
            &self.parts,
            &order,
            [
                (&first_level_indices, &pool.first_level),
                (&second_level_indices, &pool.second_level),
                (&third_level_indices, &pool.third_level),
            ],
        );
        pool.fourth_level = fourth_level;
        let (_fifth_level_indices, fifth_level) = SegmentPool::make_level(
            &self.parts,
            &order,
            [
                (&first_level_indices, &pool.first_level),
                (&second_level_indices, &pool.second_level),
                (&third_level_indices, &pool.third_level),
                (&fourth_level_indices, &pool.fourth_level),
            ],
        );
        pool.fifth_level = fifth_level;
        Ok(f(pool.run_info()))
    }

    /// Gets the list of parts specifying each segment of the given
    /// run, not including the top level one. For example, consider
    /// the run (((AB)C)D). These four segments would be returned as
    /// - [ABCD, ABC, AB, A]
    /// - [ABCD, ABC, AB, B]
    /// - [ABCD, ABC, C]
    /// - [ABCD, D]
    ///
    /// Note that there is an implied ABCD
    pub fn nested_segment_names(
        &self,
        id_name: &str,
    ) -> Result<Vec<Vec<String>>> {
        let inner = self.use_run_info(id_name, |run_part| {
            run_part.nested_segment_names()
        })?;
        match self.parts.get(id_name).unwrap() {
            ConfigPart::Segment(segment_info) => {
                Ok(vec![vec![segment_info.display_name.clone()]])
            }
            ConfigPart::Group(segment_group_info) => {
                let mut result = Vec::with_capacity(inner.len());
                let display_name = &segment_group_info.display_name;
                for parts in inner {
                    let mut segment = Vec::with_capacity(parts.len() + 1);
                    segment.push(display_name.clone());
                    for part in parts {
                        segment.push(part.display_name);
                    }
                    result.push(segment)
                }
                Ok(result)
            }
        }
    }

    pub fn run(
        &self,
        id_name: &str,
        segment_run_event_receiver: Receiver<SegmentRunEvent>,
        supplemented_segment_run_sender: Sender<SupplementedSegmentRun>,
        write: bool,
    ) -> Result<()> {
        match self.use_run_info(id_name, |run_part| {
            run_part.run(
                segment_run_event_receiver,
                supplemented_segment_run_sender,
                write,
            )
        }) {
            Ok(result) => match result {
                Ok(()) => Ok(()),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    }
}

/// Command line input to any binary
pub struct Input {
    /// positional arguments supplied from the command line (binary name is first)
    arguments: Vec<String>,
    /// named arguments supplied from the command line
    options: HashMap<String, Vec<String>>,
}

impl Input {
    /// Creates a new empty input
    fn new() -> Self {
        Input {
            arguments: Vec::new(),
            options: HashMap::new(),
        }
    }

    /// Gets options and arguments from the given iterator over command line args
    pub fn collect<T: IntoIterator<Item = String>>(args: T) -> Result<Self> {
        let mut input = Self::new();
        let mut current_option: Option<(String, Vec<String>)> = None;
        for value in args {
            current_option = match current_option {
                Some((current_key, mut current_values)) => {
                    if value.starts_with("--") {
                        let current_key_clone = current_key.clone();
                        if let Some(_) =
                            input.options.insert(current_key, current_values)
                        {
                            return Err(Error::OptionProvidedTwice {
                                option: current_key_clone,
                            });
                        }
                        Some((value, Vec::new()))
                    } else {
                        current_values.push(value);
                        Some((current_key, current_values))
                    }
                }
                None => {
                    if value.starts_with("--") {
                        Some((value, Vec::new()))
                    } else {
                        input.arguments.push(value);
                        None
                    }
                }
            }
        }
        if let Some((final_key, final_values)) = current_option {
            let final_key_clone = final_key.clone();
            if let Some(_) = input.options.insert(final_key, final_values) {
                return Err(Error::OptionProvidedTwice {
                    option: final_key_clone,
                });
            }
        }
        Ok(input)
    }

    /// Provides a view to the list of positional arguments provided from command line
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Provides a view to the mapping from option name to option values
    pub fn options(&self) -> &HashMap<String, Vec<String>> {
        &self.options
    }

    /// Removes and returns the entry of the options with the given name
    pub fn extract_option(
        &mut self,
        option_name: &str,
    ) -> Option<(String, Vec<String>)> {
        self.options.remove_entry(option_name)
    }

    /// Removes the given option, returning true if it is given and false otherwise.
    /// Returns an error if the option has values associated with it.
    pub fn extract_option_no_values(
        &mut self,
        option_name: &str,
    ) -> Result<bool> {
        match self.extract_option(option_name) {
            Some((option, values)) => match values.is_empty() {
                true => Ok(true),
                false => Err(Error::OptionExpectedNoValues { option, values }),
            },
            None => Ok(false),
        }
    }

    /// Gets the single value associated with the given option name. Returns error if option
    /// is not given or a different number of values than 1 is associated with it.
    pub fn extract_option_single_value(
        &mut self,
        option_name: &str,
    ) -> Result<String> {
        match self.extract_option(option_name) {
            Some((option, mut values)) => {
                if values.len() != 1 {
                    Err(Error::OptionExpectedOneValue { option, values })
                } else {
                    Ok(values.pop().unwrap())
                }
            }
            None => Err(Error::OptionNotFound {
                option: String::from(option_name),
            }),
        }
    }

    /// Gets the root directory to load the config from. Removes the --root option, if it exists
    pub fn extract_root(
        &mut self,
        home_directory: Option<String>,
    ) -> Result<PathBuf> {
        match self.extract_option_single_value("--root") {
            Ok(root) => Ok(PathBuf::from(root)),
            Err(error) => match &error {
                Error::OptionExpectedOneValue { .. } => Err(error),
                _ => match home_directory {
                    Some(home) => {
                        let mut root = PathBuf::from(home);
                        root.push("srtim");
                        Ok(root)
                    }
                    None => Err(Error::NoRootFound),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;
    use std::fs::remove_file;
    use std::ptr;
    use std::sync::mpsc;
    use std::thread;

    /// Ensures that the now() associated function of MillisecondsSinceEpoch gives something similar to current time.
    #[test]
    fn milliseconds_now() {
        assert!(MillisecondsSinceEpoch::now().0 > 1_741_757_101_000);
    }

    /// Tests that the MillisecondsSinceEpoch::duration method correctly
    /// subtracts times in the right order (i.e. end - start)
    #[test]
    fn positive_duration() {
        let start = MillisecondsSinceEpoch(10);
        let end = MillisecondsSinceEpoch(20);
        assert_eq!(
            MillisecondsSinceEpoch::duration(start, end)
                .unwrap()
                .as_millis(),
            10
        );
    }

    /// Tests that the MillisecondsSinceEpoch::duration method correctly
    /// subtracts times in the right order (i.e. end - start)
    #[test]
    fn zero_duration() {
        let instant = MillisecondsSinceEpoch(10);
        assert!(
            MillisecondsSinceEpoch::duration(instant, instant)
                .unwrap()
                .is_zero()
        );
    }

    /// Tests that the NegativeDuration error is returned
    /// if end < start in negative_duration
    #[test]
    fn negative_duration() {
        let start = MillisecondsSinceEpoch(10);
        let end = MillisecondsSinceEpoch(0);
        assert!(MillisecondsSinceEpoch::duration(start, end).is_err_and(
            |error| match error {
                Error::NegativeDuration { start: 10, end: 0 } => true,
                _ => false,
            }
        ));
    }

    /// Tests that segment runs can be combined if one starts when the other ends
    #[test]
    fn combine_segment_runs() {
        let mut accumulator = SegmentRun {
            deaths: 1,
            start: MillisecondsSinceEpoch(0),
            end: MillisecondsSinceEpoch(10),
        };
        let new = SegmentRun {
            deaths: 2,
            start: MillisecondsSinceEpoch(10),
            end: MillisecondsSinceEpoch(30),
        };

        accumulator.accumulate(&new).unwrap();
        assert_eq!(accumulator.deaths, 3);
        assert_eq!(accumulator.start.0, 0);
        assert_eq!(accumulator.end.0, 30);
        assert_eq!(accumulator.duration(), Duration::from_millis(30));
    }

    /// Tests that an error is returned when attempting to combine segment runs that have time between them
    #[test]
    fn combine_segment_runs_no_shared_endpoint() {
        let mut existing = SegmentRun {
            deaths: 1,
            start: MillisecondsSinceEpoch(0),
            end: MillisecondsSinceEpoch(10),
        };
        let clone = existing.clone();
        assert!(existing.accumulate(&clone).is_err_and(|error| match error {
            Error::SegmentCombinationError => true,
            _ => false,
        }));
    }

    /// Ensures that the duration of a segment run is correctly computed from difference of endpoints
    #[test]
    fn duration_of_segment_run() {
        let segment_run = SegmentRun {
            deaths: 0,
            start: MillisecondsSinceEpoch(0),
            end: MillisecondsSinceEpoch(500_000),
        };
        let duration = segment_run.duration();
        let expected_duration = Duration::from_secs(500);
        assert_eq!(duration, expected_duration);
    }

    /// Thin wrapper around a PathBuf that removes any file at the path upon drop
    struct TempFile {
        /// the file being tracked which will be deleted upon dropping
        path: PathBuf,
    }

    impl TempFile {
        /// tracks a new path for deletion at the dropping of this object
        fn new(path: PathBuf) -> Self {
            Self { path }
        }

        /// writes a new file with the given contents and returns
        /// a TempFile that will remove it upon being dropped
        fn with_contents<'b>(
            path: PathBuf,
            contents: &'b str,
        ) -> std::result::Result<Self, io::Error> {
            fs::write(&path, contents)?;
            Ok(TempFile { path })
        }
    }

    impl Drop for TempFile {
        /// when this object goes out of scope, the temp file should be deleted
        fn drop(&mut self) {
            match remove_file(&self.path) {
                _ => {}
            }
        }
    }

    /// Tests that load_all loads segment runs as expected
    #[test]
    fn load_all_segment_runs_happy_path() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_happy_path.csv"),
            "start,end,deaths\n0,100,10\n1000,2000,2\n",
        )
        .unwrap();
        let segment_runs = SegmentRun::load_all(&temp_file.path).unwrap();
        assert_eq!(segment_runs.len(), 2);
        let segment_run = segment_runs[0];
        assert_eq!(segment_run.deaths, 10);
        assert_eq!(segment_run.duration(), Duration::from_millis(100));
        let segment_run = segment_runs[1];
        assert_eq!(segment_run.deaths, 2);
        assert_eq!(segment_run.duration(), Duration::from_millis(1000));
    }

    /// Tests that the CouldNotReadSegmentRunFile error
    /// is returned if a non-existent file is passed
    #[test]
    fn load_all_segment_runs_nonexistent_file() {
        let temp =
            temp_dir().join("load_all_segment_runs_nonexistent_file.csv");
        assert!(SegmentRun::load_all(&temp).is_err_and(|error| match error {
            Error::CouldNotReadSegmentRunFile { path, error } => {
                (temp == path) && (error.kind() == io::ErrorKind::NotFound)
            }
            _ => false,
        }));
    }

    /// Tests that a SegmentRunFileLineParseError is returned if
    /// a column has a token that isn't parsable as an integer.
    #[test]
    fn load_all_segment_runs_unparsable() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_0.csv"),
            "start,end,deaths\n0,10,0\n15.,20,1\n",
        )
        .unwrap();
        assert!(SegmentRun::load_all(&temp_file.path).is_err_and(|error| {
            match error {
                Error::SegmentRunFileLineParseError {
                    index: 2,
                    column: 0,
                } => true,
                _ => false,
            }
        }));
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_1.csv"),
            "start,end,deaths\n0,1h,0\n",
        )
        .unwrap();
        assert!(SegmentRun::load_all(&temp_file.path).is_err_and(|error| {
            match error {
                Error::SegmentRunFileLineParseError {
                    index: 1,
                    column: 1,
                } => true,
                _ => false,
            }
        }));
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_2.csv"),
            "start,end,deaths\n0,10,0.\n",
        )
        .unwrap();
        assert!(SegmentRun::load_all(&temp_file.path).is_err_and(|error| {
            match error {
                Error::SegmentRunFileLineParseError {
                    index: 1,
                    column: 2,
                } => true,
                _ => false,
            }
        }));
    }

    /// Tests that load_all fails to load segment runs if there are too many or too few comas on a row
    #[test]
    fn load_all_segment_runs_too_many_commas() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_too_many_commas.csv"),
            "start,end,deaths\n0,10,10,\n",
        )
        .unwrap();
        assert!(SegmentRun::load_all(&temp_file.path).is_err_and(|error| {
            match error {
                Error::InvalidSegmentRunFileLine {
                    index: 1,
                    too_many_separators: true,
                    expected_number_of_elements: 3,
                } => true,
                _ => false,
            }
        }));
    }

    /// Tests that load_all fails to load segment runs if there are too many or too few comas on a row
    #[test]
    fn load_all_segment_runs_not_enough_commas() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_not_enough_commas.csv"),
            "start,end,deaths\n0,10\n",
        )
        .unwrap();
        assert!(SegmentRun::load_all(&temp_file.path).is_err_and(|error| {
            match error {
                Error::InvalidSegmentRunFileLine {
                    index: 1,
                    too_many_separators: false,
                    expected_number_of_elements: 3,
                } => true,
                _ => false,
            }
        }));
    }

    /// Ensures that SegmentInfo requires an absolute path file_name
    #[test]
    fn segment_info_no_absolute_path() {
        assert!(
            SegmentInfo::new(String::from("A"), PathBuf::from("A.txt"))
                .is_err()
        );
    }

    /// Ensures that SegmentGroupInfo requires an absolute path file_name
    #[test]
    fn segment_group_info_no_absolute_path() {
        assert!(
            SegmentGroupInfo::new(
                String::from("A"),
                vec![String::from("a")],
                PathBuf::from("A.txt")
            )
            .is_err()
        );
    }

    /// Ensures that SegmentGroupInfo requires a non-empty list of parts.
    #[test]
    fn segment_group_info_no_parts() {
        assert!(
            SegmentGroupInfo::new(
                String::from("A"),
                Vec::new(),
                PathBuf::from("/A.txt")
            )
            .is_err()
        )
    }

    /// Tests that the zip_same iterator:
    /// 1. yields identical elements
    /// 2. yields None forever after being depleted
    /// 3. does not yield from the source iterators more than it needs to
    #[test]
    fn test_zip_same() {
        let mut x_yielded: Vec<i32> = vec![];
        let mut y_yielded: Vec<i32> = vec![];
        let x = [1, 2, 3, 4].into_iter().map(|value| {
            x_yielded.push(value);
            value
        });
        let y = vec![1, 2, 5, 4, 6].into_iter().map(|value| {
            y_yielded.push(value);
            value
        });
        let mut z = zip_same(x, y);
        assert_eq!(z.next().unwrap(), 1);
        assert_eq!(z.next().unwrap(), 2);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
        drop(z);
        assert_eq!(x_yielded.len(), 3);
        let mut x_yielded_iter = x_yielded.into_iter();
        assert_eq!(x_yielded_iter.next().unwrap(), 1);
        assert_eq!(x_yielded_iter.next().unwrap(), 2);
        assert_eq!(x_yielded_iter.next().unwrap(), 3);
        assert_eq!(y_yielded.len(), 3);
        let mut y_yielded_iter = y_yielded.into_iter();
        assert_eq!(y_yielded_iter.next().unwrap(), 1);
        assert_eq!(y_yielded_iter.next().unwrap(), 2);
        assert_eq!(y_yielded_iter.next().unwrap(), 5);
    }

    /// Collects references to all of the SegmentInfo objects of low-level segments into a vector
    fn run_part_segments<'a>(part: &'a RunPart) -> Vec<&'a SegmentInfo> {
        match part {
            &RunPart::Group { .. } => part
                .nested_segment_specs()
                .iter()
                .map(|s| match s.last().unwrap().1 {
                    &RunPart::SingleSegment(segment) => segment,
                    _ => panic!("This can't happen, compiler!"),
                })
                .collect(),
            &RunPart::SingleSegment(segment) => vec![segment],
        }
    }

    /// Test making a run with 5 segments, ABCDE where ABC and DE are sub groups.
    #[test]
    fn make_run() {
        let a = SegmentInfo::new(String::from("A"), PathBuf::from("/A.txt"))
            .unwrap();
        let ap = RunPart::SingleSegment(&a);
        let b = SegmentInfo::new(String::from("B"), PathBuf::from("/B.txt"))
            .unwrap();
        let bp = RunPart::SingleSegment(&b);
        let c = SegmentInfo::new(String::from("C"), PathBuf::from("/C.txt"))
            .unwrap();
        let cp = RunPart::SingleSegment(&c);
        let d = SegmentInfo::new(String::from("D"), PathBuf::from("/D.txt"))
            .unwrap();
        let dp = RunPart::SingleSegment(&d);
        let e = SegmentInfo::new(String::from("E"), PathBuf::from("/E.txt"))
            .unwrap();
        let ep = RunPart::SingleSegment(&e);
        let abc_file_name = PathBuf::from("ABC.txt");
        let abcp = RunPart::Group {
            components: vec![&ap, &bp, &cp],
            display_name: "ABC",
            file_name: &abc_file_name,
        };
        let de_file_name = PathBuf::from("DE.txt");
        let dep = RunPart::Group {
            components: vec![&dp, &ep],
            display_name: "DE",
            file_name: &de_file_name,
        };
        let abcde_file_name = PathBuf::from("ABCDE.txt");
        let run = RunPart::Group {
            components: vec![&abcp, &dep],
            display_name: "ABCDE",
            file_name: &abcde_file_name,
        };
        let mut iter = run_part_segments(&run).into_iter();
        assert!(ptr::eq(iter.next().unwrap(), &a));
        assert!(ptr::eq(iter.next().unwrap(), &b));
        assert!(ptr::eq(iter.next().unwrap(), &c));
        assert!(ptr::eq(iter.next().unwrap(), &d));
        assert!(ptr::eq(iter.next().unwrap(), &e));
        assert!(iter.next().is_none());
    }

    /// Assembles a run that looks like (((AB)C)D) where the letters are individual segments and the
    /// parentheses indicate groups. Then, tests ensure that the RunInfo::nested_segment_specs and
    /// RunInfo::segments function produce the expected references.
    #[test]
    fn nested_segment_specs_test() {
        let a = SegmentInfo::new(String::from("A"), PathBuf::from("/A.txt"))
            .unwrap();
        let ap = RunPart::SingleSegment(&a);
        let b = SegmentInfo::new(String::from("B"), PathBuf::from("/B.txt"))
            .unwrap();
        let bp = RunPart::SingleSegment(&b);
        let c = SegmentInfo::new(String::from("C"), PathBuf::from("/C.txt"))
            .unwrap();
        let cp = RunPart::SingleSegment(&c);
        let d = SegmentInfo::new(String::from("D"), PathBuf::from("/D.txt"))
            .unwrap();
        let dp = RunPart::SingleSegment(&d);
        let abf = PathBuf::from("/AB.txt");
        let abp = RunPart::Group {
            components: vec![&ap, &bp],
            display_name: "AB",
            file_name: &abf,
        };
        let abcf = PathBuf::from("/ABC.txt");
        let abcp = RunPart::Group {
            components: vec![&abp, &cp],
            display_name: "ABC",
            file_name: &abcf,
        };
        let abcdf = PathBuf::from("/ABCD.txt");
        let run_part = RunPart::Group {
            components: vec![&abcp, &dp],
            display_name: "ABCD",
            file_name: &abcdf,
        };
        let mut segment_specs = run_part.nested_segment_specs().into_iter();
        let mut a_spec = segment_specs.next().unwrap().into_iter();
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        match a_part {
            &RunPart::Group { display_name, .. } => {
                assert_eq!(display_name, "ABC");
            }
            _ => panic!("expected group, got segment"),
        }
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        match a_part {
            &RunPart::Group { display_name, .. } => {
                assert_eq!(display_name, "AB");
            }
            _ => panic!("expected group, got segment"),
        }
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        match a_part {
            &RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                assert_eq!(display_name, "A");
            }
            _ => panic!("expected segment, got group"),
        }
        assert!(a_spec.next().is_none());
        let mut b_spec = segment_specs.next().unwrap().into_iter();
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 0);
        match b_part {
            &RunPart::Group { display_name, .. } => {
                assert_eq!(display_name, "ABC");
            }
            _ => panic!("expected group, got segment"),
        }
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 0);
        match b_part {
            &RunPart::Group { display_name, .. } => {
                assert_eq!(display_name, "AB");
            }
            _ => panic!("expected group, got segment"),
        }
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 1);
        match b_part {
            &RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                assert_eq!(display_name, "B");
            }
            _ => panic!("expected segment, got group"),
        }
        assert!(b_spec.next().is_none());
        let mut c_spec = segment_specs.next().unwrap().into_iter();
        let (index, c_part) = c_spec.next().unwrap();
        assert_eq!(index, 0);
        match c_part {
            &RunPart::Group { display_name, .. } => {
                assert_eq!(display_name, "ABC");
            }
            _ => panic!("expected group, got segment"),
        }
        let (index, c_part) = c_spec.next().unwrap();
        assert_eq!(index, 1);
        match c_part {
            &RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                assert_eq!(display_name, "C");
            }
            _ => panic!("expected segment, got group"),
        }
        assert!(c_spec.next().is_none());
        let mut d_spec = segment_specs.next().unwrap().into_iter();
        let (index, d_part) = d_spec.next().unwrap();
        assert_eq!(index, 1);
        match d_part {
            &RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                assert_eq!(display_name, "D");
            }
            _ => panic!("expected segment, got group"),
        }
        assert!(d_spec.next().is_none());
        let mut segments = run_part_segments(&run_part).into_iter();
        match a_part {
            &RunPart::SingleSegment(segment) => {
                assert!(ptr::eq(segment, segments.next().unwrap()))
            }
            _ => panic!("expected segment, got group"),
        }
        match b_part {
            &RunPart::SingleSegment(segment) => {
                assert!(ptr::eq(segment, segments.next().unwrap()))
            }
            _ => panic!("expected segment, got group"),
        }
        match c_part {
            &RunPart::SingleSegment(segment) => {
                assert!(ptr::eq(segment, segments.next().unwrap()))
            }
            _ => panic!("expected segment, got group"),
        }
        match d_part {
            &RunPart::SingleSegment(segment) => {
                assert!(ptr::eq(segment, segments.next().unwrap()))
            }
            _ => panic!("expected segment, got group"),
        }
        assert!(segments.next().is_none());
    }

    /// Tests the error that is returned when the run_receiver is dropped while running run_consecutive
    #[test]
    fn run_one_segment_closed_run_receiver() {
        let (event_sender, event_receiver) = mpsc::channel();
        let (run_sender, run_receiver) = mpsc::channel();
        drop(run_receiver);
        let receive_events = thread::spawn(move || {
            assert!(
                SegmentRun::run_consecutive(1, event_receiver, run_sender)
                    .is_err_and(|error| {
                        match error {
                            Error::FailedToSendSegment { index: 1u32 } => true,
                            _ => false,
                        }
                    })
            );
        });
        event_sender.send(SegmentRunEvent::End).unwrap();
        receive_events.join().unwrap();
    }

    /// Tests that the correct error is returned if the event sender
    /// is closed before enough events are sent to complete a run
    #[test]
    fn run_one_segment_closed_event_sender() {
        let (event_sender, event_receiver) = mpsc::channel();
        let (run_sender, _) = mpsc::channel();
        drop(event_sender);
        thread::spawn(move || {
            assert!(
                SegmentRun::run_consecutive(1, event_receiver, run_sender)
                    .is_err_and(|error| {
                        match error {
                            Error::ErrorDuringSegment {
                                index: 1u32,
                                error: sub_error,
                            } => match sub_error.as_ref() {
                                Error::ChannelClosedBeforeEnd => true,
                                _ => false,
                            },
                            _ => false,
                        }
                    })
            )
        })
        .join()
        .unwrap();
    }

    /// Runs two segments consecutively using the `SegmentRun::run_consecutive` function
    /// Asserts that death count is as expected by number of death events sent and that
    /// durations of segments have sleep durations as lower bounds.
    #[test]
    fn run_two_segments() {
        let (event_sender, event_receiver) = mpsc::channel();
        let (run_sender, run_receiver) = mpsc::channel();
        let receive_segments = thread::spawn(move || {
            let mut segment_runs: Vec<SegmentRun> = Vec::new();
            while let Ok(segment_run) = run_receiver.recv() {
                segment_runs.push(segment_run);
            }
            segment_runs
        });
        let receive_events = thread::spawn(move || {
            SegmentRun::run_consecutive(2, event_receiver, run_sender).unwrap()
        });
        thread::sleep(Duration::from_millis(10));
        event_sender.send(SegmentRunEvent::Death).unwrap();
        thread::sleep(Duration::from_millis(5));
        event_sender.send(SegmentRunEvent::Death).unwrap();
        thread::sleep(Duration::from_millis(15));
        event_sender.send(SegmentRunEvent::End).unwrap();
        thread::sleep(Duration::from_millis(40));
        event_sender.send(SegmentRunEvent::End).unwrap();
        receive_events.join().unwrap();
        let mut segment_runs = receive_segments.join().unwrap().into_iter();
        let segment_run = segment_runs.next().unwrap();
        assert_eq!(segment_run.deaths, 2);
        assert!(segment_run.duration().as_millis() >= 30);
        let segment_run = segment_runs.next().unwrap();
        assert_eq!(segment_run.deaths, 0);
        assert!(segment_run.duration().as_millis() >= 40);
        assert!(segment_runs.next().is_none());
    }

    /// Tests the load_n_tokens function in the case where there are more
    /// tokens than expected. In this case Err(true) should be returned.
    #[test]
    fn load_n_tokens_too_many() {
        assert!(
            load_n_tokens("hello,world".split(','), 1)
                .is_err_and(|too_many_commas| too_many_commas)
        );
    }

    /// Tests the load_n_tokens function in the case where there are fewer
    /// tokens than expected. In this case Err(false) should be returned.
    #[test]
    fn load_n_tokens_not_enough() {
        assert!(
            load_n_tokens("hello,world,howdy".split(','), 4)
                .is_err_and(|too_many_commas| !too_many_commas)
        );
    }

    /// Tests the load_n_tokens function in the case where there are more
    /// tokens than expected. In this case Err(true) should be returned.
    #[test]
    fn load_n_tokens_correct_number() {
        let vector =
            load_n_tokens("hello,world,my,name,is,,Keith".split(','), 7)
                .unwrap();
        let mut tokens = vector.into_iter();
        assert_eq!(tokens.next().unwrap(), "hello");
        assert_eq!(tokens.next().unwrap(), "world");
        assert_eq!(tokens.next().unwrap(), "my");
        assert_eq!(tokens.next().unwrap(), "name");
        assert_eq!(tokens.next().unwrap(), "is");
        assert_eq!(tokens.next().unwrap(), "");
        assert_eq!(tokens.next().unwrap(), "Keith");
        assert!(tokens.next().is_none());
    }

    /// Makes a config containing 5 segments, A-E, and three groups, (AB), ((AB)C), and (((AB)C)D)
    fn make_abcd_config(extra: &str) -> (Config, TempFile) {
        let mut config = Config::new(temp_dir().join(extra));
        assert!(
            config
                .add_segment(String::from("b"), String::from("B"))
                .is_ok()
        );
        assert!(
            config
                .add_segment(String::from("a"), String::from("A"))
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    String::from("ab"),
                    String::from("AB"),
                    vec![String::from("a"), String::from("b")]
                )
                .is_ok()
        );
        assert!(
            config
                .add_segment(String::from("d"), String::from("D"))
                .is_ok()
        );
        assert!(
            config
                .add_segment(String::from("c"), String::from("C"))
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    String::from("abc"),
                    String::from("ABC"),
                    vec![String::from("ab"), String::from("c")]
                )
                .is_ok()
        );
        assert!(
            config
                .add_segment(String::from("e"), String::from("E"))
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    String::from("abcd"),
                    String::from("ABCD"),
                    vec![String::from("abc"), String::from("d")]
                )
                .is_ok()
        );
        assert!(
            config
                .add_segment(String::from("f"), String::from("F"))
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    String::from("abcdf"),
                    String::from("ABCDF"),
                    vec![String::from("abcd"), String::from("f")]
                )
                .is_ok()
        );
        assert!(
            config
                .add_segment(String::from("g"), String::from("G"))
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    String::from("abcdfg"),
                    String::from("ABCDFG"),
                    vec![String::from("abcdf"), String::from("g")]
                )
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    String::from("ababc"),
                    String::from("ABABC"),
                    vec![String::from("ab"), String::from("abc")]
                )
                .is_ok()
        );
        let temp_file = TempFile::new(config.root.join("config.tsv"));
        drop(temp_file);
        let temp_file = TempFile::new(config.root.join("config.tsv"));
        (config, temp_file)
    }

    /// Tests the public Config::nested_segment_names function to ensure
    /// that segments are represented as a single list of a single string
    #[test]
    fn test_config_nested_segment_names_segment() {
        let config = make_abcd_config("A").0;
        let mut nested_segment_names =
            config.nested_segment_names("a").unwrap().into_iter();
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "A");
        assert!(inner_segment_names.next().is_none());
        assert!(nested_segment_names.next().is_none());
    }

    /// Tests the public Config::nested_segment_names function
    /// to ensure that groups are represented correctly.
    #[test]
    fn test_config_nested_segment_names_group() {
        let config = make_abcd_config("B").0;
        let mut nested_segment_names =
            config.nested_segment_names("abcd").unwrap().into_iter();
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABCD");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABC");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "AB");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "A");
        assert!(inner_segment_names.next().is_none());
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABCD");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABC");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "AB");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "B");
        assert!(inner_segment_names.next().is_none());
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABCD");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABC");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "C");
        assert!(inner_segment_names.next().is_none());
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "ABCD");
        assert_eq!(inner_segment_names.next().unwrap().as_str(), "D");
        assert!(inner_segment_names.next().is_none());
        assert!(nested_segment_names.next().is_none());
    }

    #[test]
    fn test_config_save_and_load() {
        let (config, _temp_file) = make_abcd_config("a");
        config.save().unwrap();
        let loaded = Config::load(config.root.clone()).unwrap();
        assert_eq!(config.parts.len(), loaded.parts.len());
        assert!(
            config
                .parts
                .keys()
                .all(|part_name| loaded.parts.contains_key(part_name))
        );
    }

    /// Tests that the Config struct's associated function order():
    /// 1. Returns None if the ID name supplied to it doesn't correspond to a part
    /// 2. Works when called on a single segment (a hash map with one value (id_name, 0))
    /// 3. Fills out order maps of full run trees
    #[test]
    fn test_order() {
        let (config, _temp_file) = make_abcd_config("b");
        assert!(config.order("abcde").is_none());
        let order = config.order("abcd").unwrap();
        assert_eq!(order.len(), 7);
        assert_eq!(order.get("a").unwrap(), &0);
        assert_eq!(order.get("b").unwrap(), &0);
        assert_eq!(order.get("c").unwrap(), &0);
        assert_eq!(order.get("d").unwrap(), &0);
        assert_eq!(order.get("ab").unwrap(), &1);
        assert_eq!(order.get("abc").unwrap(), &2);
        assert_eq!(order.get("abcd").unwrap(), &3);
        assert!(order.get("e").is_none());
        let order = config.order("e").unwrap();
        assert_eq!(order.len(), 1);
        assert_eq!(order.get("e").unwrap(), &0);
        let order = config.order("ababc").unwrap();
        assert_eq!(order.len(), 6);
        assert_eq!(order.get("a").unwrap(), &0);
        assert_eq!(order.get("b").unwrap(), &0);
        assert_eq!(order.get("c").unwrap(), &0);
        assert_eq!(order.get("ab").unwrap(), &1);
        assert_eq!(order.get("abc").unwrap(), &2);
        assert_eq!(order.get("ababc").unwrap(), &3);
    }

    /// Tests the use_run_info method on the config class on a run with three levels of nesting (((AB)C)D)
    #[test]
    fn test_run_info_four_levels() {
        let (config, _temp_file) = make_abcd_config("c");
        let group_id_name = "abcd";
        let mut segment_names = config
            .use_run_info(group_id_name, |run_info| {
                run_info
                    .nested_segment_specs()
                    .into_iter()
                    .map(|v| {
                        let mut output: String =
                            String::from(run_info.display_name());
                        for (_, p) in v {
                            output.push(':');
                            output.push_str(match p {
                                RunPart::SingleSegment(SegmentInfo {
                                    display_name,
                                    ..
                                }) => display_name.as_str(),
                                RunPart::Group { display_name, .. } => {
                                    display_name
                                }
                            });
                        }
                        output
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap()
            .into_iter();
        assert_eq!(segment_names.next().unwrap().as_str(), "ABCD:ABC:AB:A");
        assert_eq!(segment_names.next().unwrap().as_str(), "ABCD:ABC:AB:B");
        assert_eq!(segment_names.next().unwrap().as_str(), "ABCD:ABC:C");
        assert_eq!(segment_names.next().unwrap().as_str(), "ABCD:D");
        assert!(segment_names.next().is_none());
    }

    /// Tests the use_run_info method on the config class on a run with two levels of nesting ((AB)C)
    #[test]
    fn test_run_info_three_levels() {
        let (config, _temp_file) = make_abcd_config("d");
        let group_id_name = "abc";
        let mut segment_names = config
            .use_run_info(group_id_name, |run_info| {
                run_info
                    .nested_segment_specs()
                    .into_iter()
                    .map(|v| {
                        let mut output: String =
                            String::from(run_info.display_name());
                        for (_, p) in v {
                            output.push(':');
                            output.push_str(match p {
                                RunPart::SingleSegment(SegmentInfo {
                                    display_name,
                                    ..
                                }) => display_name.as_str(),
                                RunPart::Group { display_name, .. } => {
                                    display_name
                                }
                            });
                        }
                        output
                    })
                    .collect::<Vec<String>>()
            })
            .unwrap()
            .into_iter();
        assert_eq!(segment_names.next().unwrap().as_str(), "ABC:AB:A");
        assert_eq!(segment_names.next().unwrap().as_str(), "ABC:AB:B");
        assert_eq!(segment_names.next().unwrap().as_str(), "ABC:C");
        assert!(segment_names.next().is_none());
    }

    /// Tests that the use_run_info function on the Config class correctly
    /// assembles a run tree with just a single segment as its root
    #[test]
    fn test_run_info_single_segment() {
        let (config, _temp_file) = make_abcd_config("e");
        let group_id_name = "e";
        let mut segment_names = config
            .use_run_info(group_id_name, |run_info| {
                assert!(run_info.nested_segment_specs().is_empty());
                run_part_segments(&run_info)
                    .into_iter()
                    .map(|s| String::from(&s.display_name))
                    .collect::<Vec<String>>()
            })
            .unwrap()
            .into_iter();
        assert_eq!(segment_names.next().unwrap().as_str(), "E");
        assert!(segment_names.next().is_none());
    }

    /// Uses the RunPart::run function to initiate and complete a timed run of a single segment.
    #[test]
    fn test_single_segment_run_part() {
        let (config, _temp_file) = make_abcd_config("f");
        let group_id_name = "e";
        let group_display_name = "E";
        let (segment_run_event_sender, segment_run_event_receiver) =
            mpsc::channel();
        let (
            supplemented_segment_run_sender,
            supplemented_segment_run_receiver,
        ) = mpsc::channel();
        let run_event_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            assert!(
                segment_run_event_sender.send(SegmentRunEvent::End).is_err()
            );
        });
        let run_thread = thread::spawn(move || {
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(segment_run.deaths, 2);
            assert!(segment_run.duration() >= Duration::from_millis(40));
            assert_eq!(&name, group_display_name);
            assert_eq!(nesting, 0);
            assert!(supplemented_segment_run_receiver.recv().is_err());
        });
        config
            .run(
                group_id_name,
                segment_run_event_receiver,
                supplemented_segment_run_sender,
                false,
            )
            .unwrap();
        run_event_thread.join().unwrap();
        run_thread.join().unwrap();
    }

    /// Uses the RunPart::run function to initiate and complete a timed run of a segment group.
    #[test]
    fn test_multi_segment_run_part() {
        let (config, _temp_file) = make_abcd_config("g");
        config.save().unwrap();
        let group_id_name = "ababc";
        let (segment_run_event_sender, segment_run_event_receiver) =
            mpsc::channel();
        let (
            supplemented_segment_run_sender,
            supplemented_segment_run_receiver,
        ) = mpsc::channel();
        let run_event_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(15));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(15));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            thread::sleep(Duration::from_millis(20));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(15));
            assert!(
                segment_run_event_sender.send(SegmentRunEvent::End).is_err()
            );
        });
        let run_thread = thread::spawn(move || {
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name, "A");
            assert_eq!(segment_run.deaths, 1);
            assert!(segment_run.duration() >= Duration::from_millis(30));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name, "B");
            assert_eq!(segment_run.deaths, 2);
            assert!(segment_run.duration() >= Duration::from_millis(35));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 1);
            assert_eq!(&name, "AB");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(65));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 3);
            assert_eq!(&name, "A");
            assert_eq!(segment_run.deaths, 0);
            assert!(segment_run.duration() >= Duration::from_millis(15));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 3);
            assert_eq!(&name, "B");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(50));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name, "AB");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(65));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name, "C");
            assert_eq!(segment_run.deaths, 0);
            assert!(segment_run.duration() >= Duration::from_millis(10));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 1);
            assert_eq!(&name, "ABC");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(75));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 0);
            assert_eq!(&name, "ABABC");
            assert_eq!(segment_run.deaths, 6);
            assert!(segment_run.duration() >= Duration::from_millis(140));
        });
        config
            .run(
                group_id_name,
                segment_run_event_receiver,
                supplemented_segment_run_sender,
                true,
            )
            .unwrap();
        run_event_thread.join().unwrap();
        run_thread.join().unwrap();
        let a_file = TempFile::new(config.root.join("a.csv"));
        let b_file = TempFile::new(config.root.join("b.csv"));
        let c_file = TempFile::new(config.root.join("c.csv"));
        let ab_file = TempFile::new(config.root.join("ab.csv"));
        let abc_file = TempFile::new(config.root.join("abc.csv"));
        let ababc_file = TempFile::new(config.root.join("ababc.csv"));
        let a_runs = SegmentRun::load_all(&a_file.path).unwrap();
        assert_eq!(a_runs.len(), 2);
        assert_eq!(a_runs[0].deaths, 1);
        assert!(a_runs[0].duration() >= Duration::from_millis(30));
        assert_eq!(a_runs[1].deaths, 0);
        assert!(a_runs[1].duration() >= Duration::from_millis(15));
        let b_runs = SegmentRun::load_all(&b_file.path).unwrap();
        assert_eq!(b_runs.len(), 2);
        assert_eq!(b_runs[0].deaths, 2);
        assert!(b_runs[0].duration() >= Duration::from_millis(35));
        assert_eq!(b_runs[1].deaths, 3);
        assert!(b_runs[1].duration() >= Duration::from_millis(50));
        let c_runs = SegmentRun::load_all(&c_file.path).unwrap();
        assert_eq!(c_runs.len(), 1);
        assert_eq!(c_runs[0].deaths, 0);
        assert!(c_runs[0].duration() >= Duration::from_millis(10));
        let ab_runs = SegmentRun::load_all(&ab_file.path).unwrap();
        assert_eq!(ab_runs.len(), 2);
        assert_eq!(ab_runs[0].deaths, 3);
        assert!(ab_runs[0].duration() >= Duration::from_millis(65));
        assert_eq!(ab_runs[1].deaths, 3);
        assert!(ab_runs[1].duration() >= Duration::from_millis(65));
        let abc_runs = SegmentRun::load_all(&abc_file.path).unwrap();
        assert_eq!(abc_runs.len(), 1);
        assert_eq!(abc_runs[0].deaths, 3);
        assert!(abc_runs[0].duration() >= Duration::from_millis(75));
        let ababc_runs = SegmentRun::load_all(&ababc_file.path).unwrap();
        assert_eq!(ababc_runs.len(), 1);
        assert_eq!(ababc_runs[0].deaths, 6);
        assert!(ababc_runs[0].duration() >= Duration::from_millis(140));
    }

    /// Tests that the Config::use_run_info function returns an
    /// IdNameNotFound error when a non-existent name is passed
    #[test]
    fn use_run_info_non_existent() {
        let (config, _temp_file) = make_abcd_config("h");
        assert!(config.use_run_info("z", |_| {}).is_err_and(
            |error| match error {
                Error::IdNameNotFound { id_name } => id_name.as_str() == "z",
                _ => false,
            }
        ));
    }

    /// Tests that the Config::use_run_info function returns a TooMuchNesting
    /// error when a group with more than 4 levels of nesting is passed
    #[test]
    fn use_run_info_too_nested() {
        let (config, _temp_file) = make_abcd_config("i");
        assert!(config.use_run_info("abcdfg", |_| {}).is_err_and(|error| {
            match error {
                Error::TooMuchNesting {
                    max_nesting_level: 4,
                } => true,
                _ => false,
            }
        }));
    }

    /// Tests that the RunPart::save function can both create new files and append existing ones
    #[test]
    fn save_run() {
        let path = temp_dir().join("test_run.csv");
        match fs::remove_file(&path) {
            Ok(_) => {}
            Err(error) => match error.kind() {
                io::ErrorKind::NotFound => {}
                _ => panic!("existing temp file couldn't be deleted"),
            },
        };
        let first = SegmentRun {
            deaths: 3,
            start: MillisecondsSinceEpoch(1000),
            end: MillisecondsSinceEpoch(91000),
        };
        first.save(&path).unwrap();
        assert_eq!(
            fs::read_to_string(path.clone()).unwrap().as_str(),
            "start,end,deaths\n1000,91000,3\n"
        );
        let second = SegmentRun {
            deaths: 1,
            start: MillisecondsSinceEpoch(100000),
            end: MillisecondsSinceEpoch(200000),
        };
        second.save(&path).unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap().as_str(),
            "start,end,deaths\n1000,91000,3\n100000,200000,1\n"
        );
    }

    /// Tests that the Input::collect function produces no
    /// options or arguments when no input tokens are given
    #[test]
    fn test_load_input_empty_args() {
        let Input { arguments, options } =
            Input::collect([] as [String; 0]).unwrap();
        assert!(arguments.is_empty());
        assert!(options.is_empty());
    }

    /// Tests that the Input::collect function produces the correct
    /// options when no positional arguments are passed
    #[test]
    fn test_load_input_only_options() {
        let Input {
            arguments,
            mut options,
        } = Input::collect(
            ["--option1", "--option2", "a", "b"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert!(arguments.is_empty());
        let (_, values) = options.remove_entry("--option1").unwrap();
        assert!(values.is_empty());
        let (_, values) = options.remove_entry("--option2").unwrap();
        assert_eq!(values.len(), 2);
        let mut values = values.into_iter();
        assert_eq!(values.next().unwrap().as_str(), "a");
        assert_eq!(values.next().unwrap().as_str(), "b");
        assert!(options.is_empty());
    }

    /// Tests that the Input::collect function produces the
    /// correct arguments when no options are passed.
    #[test]
    fn test_load_input_only_arguments() {
        let input =
            Input::collect(["a", "b", "c"].into_iter().map(String::from))
                .unwrap();
        assert!(input.options().is_empty());
        assert_eq!(input.arguments().len(), 3);
        assert_eq!(input.arguments()[0].as_str(), "a");
        assert_eq!(input.arguments()[1].as_str(), "b");
        assert_eq!(input.arguments()[2].as_str(), "c");
    }

    /// Tests that the Input::collect function returns the correct error when the
    /// same option is given twice (and the second instance occurs in the middle)
    #[test]
    fn test_load_input_option_given_twice_middle() {
        assert!(
            Input::collect(
                ["--option2", "--option2", "a", "b", "--option1"]
                    .into_iter()
                    .map(String::from)
            )
            .is_err_and(|error| match error {
                Error::OptionProvidedTwice { option } =>
                    option.as_str() == "--option2",
                _ => false,
            })
        );
    }

    /// Tests that the Input::collect function returns the correct error when the
    /// same option is given twice (and the second instance occurs at the end)
    #[test]
    fn test_load_input_option_given_twice_end() {
        assert!(
            Input::collect(
                ["--option1", "--option2", "a", "b", "--option2"]
                    .into_iter()
                    .map(String::from)
            )
            .is_err_and(|error| match error {
                Error::OptionProvidedTwice { option } =>
                    option.as_str() == "--option2",
                _ => false,
            })
        );
    }

    /// Tests that an Error is thrown when extract_option_no_values is called
    /// but the option asked for has values associated with it.
    #[test]
    fn test_extract_option_no_values_expected_but_one_given() {
        let mut input = Input::collect(
            ["--option", "value1", "value2"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        assert!(input.extract_option_no_values("--option").is_err_and(
            |error| match error {
                Error::OptionExpectedNoValues { option, values } => {
                    assert_eq!(option.as_str(), "--option");
                    assert_eq!(values.len(), 2);
                    assert_eq!(values[0].as_str(), "value1");
                    assert_eq!(values[1].as_str(), "value2");
                    true
                }
                _ => false,
            }
        ));
    }

    /// Tests that Ok(False) is returned when calling
    /// extract_option_no_values on an empty input
    #[test]
    fn test_extract_option_no_values_empty_input() {
        let mut input =
            Input::collect(([] as [String; 0]).into_iter()).unwrap();
        for option_name in ["--option1", "--option2", "lmkaslknd"] {
            assert!(!input.extract_option_no_values(option_name).unwrap());
        }
    }

    /// Tests that Ok(False) is returned when calling
    /// extract_option_no_values on an empty input
    #[test]
    fn test_extract_option_no_values_removes_option() {
        let mut input =
            Input::collect(([String::from("--option")]).into_iter()).unwrap();
        assert!(input.extract_option_no_values("--option").unwrap());
        assert!(!input.extract_option_no_values("--option").unwrap());
    }

    /// Tests that the Input::extract_root function returns the NoRootProvided
    /// error when --root is passed but with no associated values.
    #[test]
    fn test_extract_root_no_values_given() {
        let mut input =
            Input::collect([String::from("--root")].into_iter()).unwrap();
        assert!(input.extract_root(None).is_err_and(|error| match error {
            Error::OptionExpectedOneValue { option, values } => {
                (option.as_str() == "--root") && values.is_empty()
            }
            _ => false,
        }));
    }

    /// Tests that the Input::extract_root function returns the TooManyRootsProvided
    /// error when --root is passed with more than one associated value.
    #[test]
    fn test_extract_root_too_many_values_given() {
        let mut input = Input::collect(
            ["--root", "first", "second"].into_iter().map(String::from),
        )
        .unwrap();
        assert!(input.extract_root(None).is_err_and(|error| match error {
            Error::OptionExpectedOneValue { option, mut values } => {
                assert_eq!(option.as_str(), "--root");
                assert!(values.pop().unwrap().as_str() == "second");
                assert!(values.pop().unwrap().as_str() == "first");
                assert!(values.is_empty());
                true
            }
            _ => false,
        }));
    }

    /// Tests that the Input::extract_root function creates a path
    /// from the input value associated with the --root option.
    #[test]
    fn test_extract_root_valid() {
        let mut input = Input::collect(
            ["--root", "my_temporary_directory"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let root = input.extract_root(None).unwrap();
        assert_eq!(root, PathBuf::from("my_temporary_directory"));
    }

    /// Tests that the Input::extract_root function returns a NoRootFound
    /// error when no --root option is found and no home directory is supplied.
    #[test]
    fn test_extract_root_not_passed_empty_home() {
        let mut input = Input::new();
        assert!(input.extract_root(None).is_err_and(|error| match error {
            Error::NoRootFound => true,
            _ => false,
        }));
    }

    /// Tests that the Input::extract_root function returns ~/srtim when
    /// no --root option is found but a home directory is supplied.
    #[test]
    fn test_extract_root_not_passed_given_home() {
        let mut input = Input::new();
        let root = input.extract_root(Some(String::from("home"))).unwrap();
        let expected = PathBuf::from("home").join("srtim");
        assert_eq!(root, expected);
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_group_nonexistent_parts() {
        let (mut config, _temp_file) = make_abcd_config("j");
        let segment_group = SegmentGroupInfo {
            display_name: String::from("new"),
            file_name: PathBuf::from("/new.csv"),
            part_id_names: vec![
                String::from("nonexistent"),
                String::from("a"),
                String::from("parts"),
            ],
        };
        assert!(
            config
                .add_part(String::from("new"), ConfigPart::Group(segment_group))
                .is_err_and(|error| match error {
                    Error::PartsDontExist {
                        id_name,
                        display_name,
                        invalid_parts,
                    } => {
                        assert_eq!(id_name.as_str(), "new");
                        assert_eq!(display_name.as_str(), "new");
                        let mut invalid_parts = invalid_parts.into_iter();
                        assert_eq!(
                            invalid_parts.next().unwrap().as_str(),
                            "nonexistent"
                        );
                        assert_eq!(
                            invalid_parts.next().unwrap().as_str(),
                            "parts"
                        );
                        assert!(invalid_parts.next().is_none());
                        true
                    }
                    _ => false,
                })
        );
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_id_name_with_component_separator_or_tab() {
        let (mut config, _temp_file) = make_abcd_config("k");
        for id_name_str in ["has:colon", "has\ttab"] {
            let segment = SegmentInfo {
                display_name: String::from("new"),
                file_name: PathBuf::from("/new.csv"),
            };
            assert!(
                config
                    .add_part(
                        String::from(id_name_str),
                        ConfigPart::Segment(segment)
                    )
                    .is_err_and(|error| match error {
                        Error::IdNameInvalid { id_name } =>
                            id_name.as_str() == id_name_str,
                        _ => false,
                    })
            );
        }
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_display_name_with_tab() {
        let invalid_display_name = "has\ttab";
        assert!(
            SegmentInfo::new(
                String::from(invalid_display_name),
                PathBuf::from("/new.csv")
            )
            .is_err_and(|error| match error {
                Error::DisplayNameInvalid { display_name } =>
                    display_name.as_str() == invalid_display_name,
                _ => false,
            })
        );
        assert!(
            SegmentGroupInfo::new(
                String::from(invalid_display_name),
                vec![String::from("a")],
                PathBuf::from("/new.csv")
            )
            .is_err_and(|error| match error {
                Error::DisplayNameInvalid { display_name } =>
                    display_name.as_str() == invalid_display_name,
                _ => false,
            })
        );
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_non_unique_id_name() {
        let (mut config, _temp_file) = make_abcd_config("m");
        let segment = SegmentInfo {
            display_name: String::from("new"),
            file_name: PathBuf::from("/new.csv"),
        };
        assert!(
            config
                .add_part(String::from("a"), ConfigPart::Segment(segment))
                .is_err_and(|error| match error {
                    Error::IdNameNotUnique { id_name } =>
                        id_name.as_str() == "a",
                    _ => false,
                })
        );
    }

    /// Tests calling Config::load on a directory that doesn't have a config saved
    #[test]
    fn config_load_nonexistent_file() {
        let root: PathBuf = temp_dir().join("n");
        drop(TempFile::new(root.join("config.tsv")));
        let config = Config::load(root).unwrap();
        assert!(config.parts.is_empty());
    }

    /// Tests that the IO error that reads config file to string propagates correctly
    /// Creates a file with same name as expected config that is not UTF-8 encoded.
    #[test]
    fn config_load_unreadable_file() {
        let root: PathBuf = temp_dir().join("o");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("config.tsv"), &[0b11111111 as u8]).unwrap();
        assert!(Config::load(root).is_err_and(|error| match error {
            Error::IOErrorReadingConfig(io_error) =>
                io_error.kind() == io::ErrorKind::InvalidData,
            _ => false,
        }));
    }

    /// Tests the output representations of each error values
    #[test]
    fn test_error_representations() {
        assert_eq!(
            format!("{}", Error::NegativeDuration { start: 10, end: 0 })
                .as_str(),
            "negative duration from start=10 ms to end=0 ms"
        );
        assert_eq!(
            format!(
                "{}",
                Error::CouldNotReadFromStdin {
                    error: io::ErrorKind::NotFound.into()
                }
            ),
            format!("Couldn't read from stdin: {}", io::ErrorKind::NotFound)
        );
        assert_eq!(
            format!(
                "{}",
                Error::CouldNotReadSegmentRunFile {
                    path: PathBuf::from("file.txt"),
                    error: io::ErrorKind::NotFound.into()
                }
            ),
            format!(
                "Couldn't read contents from file.txt: {}",
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            format!(
                "{}",
                Error::InvalidSegmentRunFileLine {
                    index: 7,
                    too_many_separators: true,
                    expected_number_of_elements: 3
                }
            )
            .as_str(),
            "run entry #7 has too many commas for a segment line, which expects 2 commas."
        );
        assert_eq!(
            format!(
                "{}",
                Error::InvalidSegmentRunFileLine {
                    index: 3,
                    too_many_separators: false,
                    expected_number_of_elements: 5
                }
            )
            .as_str(),
            "run entry #3 does not have enough commas for a segment line, which expects 4 commas."
        );
        assert_eq!(
            format!(
                "{}",
                Error::SegmentRunFileLineParseError {
                    index: 102,
                    column: 2
                }
            )
            .as_str(),
            format!(
                "run entry #102 has an unparsable integer in column with index 2"
            )
        );
        assert_eq!(
            format!("{}", Error::SegmentCombinationError).as_str(),
            "cannot combine two SegmentRun objects because the later one doesn't start at the end of the earlier one"
        );
        assert_eq!(
            format!("{}", Error::ChannelClosedBeforeEnd).as_str(),
            "channel was closed before all events of a single segment could be sent"
        );
        assert_eq!(
            format!("{}", Error::FailedToSendSegment { index: 12 }).as_str(),
            "segment #12 could not be sent because the receiver was closed"
        );
        assert_eq!(
            format!(
                "{}",
                Error::FailedToSendSegmentRunEvent {
                    index: 13,
                    event: SegmentRunEvent::Death
                }
            ),
            format!(
                "segment run event #13 ({}) could not be sent because the receiver was closed",
                SegmentRunEvent::Death
            )
        );
        assert_eq!(
            format!(
                "{}",
                Error::ErrorDuringSegment {
                    index: 8,
                    error: Box::new(Error::SegmentCombinationError)
                }
            )
            .as_str(),
            "The following error was encountered in segment #8: cannot combine two SegmentRun objects because the later one doesn't start at the end of the earlier one"
        );
        assert_eq!(
            format!(
                "{}",
                Error::FileNamePathsNotAbsolute {
                    display_name: String::from("display")
                }
            )
            .as_str(),
            "file name path given to segment or segment group \"display\" not absolute"
        );
        assert_eq!(
            format!(
                "{}",
                Error::EmptySegmentGroupInfo {
                    display_name: String::from("display")
                }
            )
            .as_str(),
            "segment group \"display\" has no components"
        );
        assert_eq!(
            format!("{}", Error::SegmentRunSenderClosed).as_str(),
            "segment run sender closed before all segments were done"
        );
        assert_eq!(
            format!(
                "{}",
                Error::PartSaveError {
                    path: PathBuf::from("file.txt"),
                    error: Box::new(Error::SegmentRunSenderClosed)
                }
            )
            .as_str(),
            "encountered issue saving run info to file at file.txt: segment run sender closed before all segments were done"
        );
        assert_eq!(
            format!("{}", Error::SupplementedSegmentRunReceiverClosed).as_str(),
            "supplemented segment run receiver closed before accepting all segments!"
        );
        assert_eq!(
            format!("{}", Error::SegmentTaggingThreadPanicked).as_str(),
            "segment tagging thread panicked"
        );
        assert_eq!(
            format!(
                "{}",
                Error::DisplayNameInvalid {
                    display_name: String::from("dis\tplay")
                }
            )
            .as_str(),
            "the display name \"dis\tplay\" is invalid because it contains a tab character"
        );
        assert_eq!(
            format!(
                "{}",
                Error::IdNameInvalid {
                    id_name: String::from("i:d")
                }
            )
            .as_str(),
            "the ID name \"i:d\" is invalid because it contains either a tab or the string used as a component separator for segment groups: \":\""
        );
        assert_eq!(
            format!(
                "{}",
                Error::IdNameNotUnique {
                    id_name: String::from("a")
                }
            )
            .as_str(),
            "\"a\" is the ID name of multiple segments and/or groups. ID Names must be unique!"
        );
        assert_eq!(
            format!(
                "{}",
                Error::PartsDontExist {
                    id_name: String::from("a"),
                    display_name: String::from("A"),
                    invalid_parts: vec![String::from("b"), String::from("c")]
                }
            )
            .as_str(),
            "segment group \"a\" with display name \"A\" has the following parts that are not defined before it: [\"b\", \"c\"]"
        );
        assert_eq!(
            format!(
                "{}",
                Error::InvalidConfigLine {
                    index: 7,
                    too_many_separators: true,
                    expected_number_of_elements: 3
                }
            )
            .as_str(),
            "config entry #7 has too many tabs for a segment line, which expects 2 tabs."
        );
        assert_eq!(
            format!(
                "{}",
                Error::InvalidConfigLine {
                    index: 3,
                    too_many_separators: false,
                    expected_number_of_elements: 5
                }
            )
            .as_str(),
            "config entry #3 does not have enough tabs for a segment line, which expects 4 tabs."
        );
        assert_eq!(
            format!(
                "{}",
                Error::IOErrorReadingConfig(io::ErrorKind::NotFound.into())
            ),
            format!(
                "encountered following error reading config file: {}",
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            format!(
                "{}",
                Error::IdNameNotFound {
                    id_name: String::from("a")
                }
            )
            .as_str(),
            "\"a\" is an unknown ID name"
        );
        assert_eq!(
            format!(
                "{}",
                Error::TooMuchNesting {
                    max_nesting_level: 4
                }
            )
            .as_str(),
            "cannot handle more than 4 levels of nesting"
        );
        assert_eq!(
            format!(
                "{}",
                Error::FailedToOpenConfigFile(io::ErrorKind::NotFound.into())
            ),
            format!(
                "encountered following error opening config file to save: {}",
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            format!(
                "{}",
                Error::FailedToWriteToConfigFile(
                    io::ErrorKind::NotFound.into()
                )
            ),
            format!(
                "encountered following error writing to config file: {}",
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            format!(
                "{}",
                Error::FailedToOpenRunPartFile {
                    path: PathBuf::from("file.txt"),
                    error: io::ErrorKind::NotFound.into()
                }
            ),
            format!(
                "encountered following error opening run part file at file.txt to save: {}",
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            format!(
                "{}",
                Error::FailedToWriteToRunPartFile {
                    path: PathBuf::from("file.txt"),
                    error: io::ErrorKind::NotFound.into()
                }
            ),
            format!(
                "encountered following error writing to run part file at file.txt: {}",
                io::ErrorKind::NotFound
            )
        );
        assert_eq!(
            format!("{}", Error::NoRootFound).as_str(),
            "--root wasn't supplied, but no HOME directory is known"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionProvidedTwice {
                    option: String::from("--option")
                }
            )
            .as_str(),
            "option \"--option\" was provided twice"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionExpectedNoValues {
                    option: String::from("--option"),
                    values: vec![String::from("value")]
                }
            )
            .as_str(),
            "option \"--option\" expected no values, but got 1: [\"value\"]"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionExpectedOneValue {
                    option: String::from("--option"),
                    values: vec![]
                }
            )
            .as_str(),
            "option \"--option\" expected one value, but got 0: []"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionNotFound {
                    option: String::from("--option")
                }
            )
            .as_str(),
            "option \"--option\" not found"
        );
    }
}
