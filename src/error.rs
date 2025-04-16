//! Module containing the full crate-level error enum.
use std::fmt::{self, Debug, Display};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

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
        display_name: Arc<str>,
    },
    /// A segment group was given that had no parts
    EmptySegmentGroupInfo {
        /// display name of the empty segment group
        display_name: Arc<str>,
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
        id_name: Arc<str>,
        /// the component separator that must be avoided in ID names
        component_separator: &'static str,
    },
    /// a display name was invalid because it contained disallowed character/substring(s)
    DisplayNameInvalid {
        /// the display name of the segment or segment group that is
        display_name: Arc<str>,
    },
    /// the given ID name was not unique
    IdNameNotUnique {
        /// the ID name that already exists but was attempted to be added
        id_name: Arc<str>,
    },
    /// segment group is invalid because some parts of it don't exist yet
    PartsDontExist {
        /// ID name of the segment group
        id_name: Arc<str>,
        /// display name of the segment group
        display_name: Arc<str>,
        /// the names of the parts that don't exist
        invalid_parts: Arc<[Arc<str>]>,
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
        id_name: Arc<str>,
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
        option: Arc<str>,
    },
    /// the given option had values associated with it, even
    /// though it is expected to be a flag with no values
    OptionExpectedNoValues {
        /// the option that is meant to be a flag
        option: Arc<str>,
        /// all string values associated with the option
        values: Arc<[Arc<str>]>,
    },
    /// the given option didn't have exactly one value associated with it
    OptionExpectedOneValue {
        /// the option that is meant to have one value associated with it
        option: Arc<str>,
        /// the 0 or multuple values associated with the option
        values: Arc<[Arc<str>]>,
    },
    /// the given option that was expected was not found
    OptionNotFound {
        /// the option that was expected
        option: Arc<str>,
    },
    /// No CLI mode was found in the positional arguments
    NoModeFound,
    /// More than the mode was passed as a positional argument
    TooManyPositionalArguments { extra_argument: Arc<str> },
    /// Mode was not one of the expected values
    UnknownMode { mode: Arc<str> },
    /// Cannot delete a part because it is a (possibly
    /// transitive) member of the contained groups
    CannotDeletePart {
        part_id_name: Arc<str>,
        constraining_parts: Arc<[Arc<str>]>,
    },
}

/// Type for Result whose Error is from this library
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
            Error::FailedToSendSegmentRunEvent { index } => write!(
                f,
                "segment run event on segment #{index} could not be sent because the receiver was closed"
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
            Error::IdNameInvalid {
                id_name,
                component_separator,
            } => write!(
                f,
                "the ID name \"{id_name}\" is invalid because it contains either a tab or the string used as a component separator for segment groups: \"{component_separator}\""
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
            Error::NoModeFound => write!(
                f,
                "No CLI mode was found. Pass it as positional argument"
            ),
            Error::TooManyPositionalArguments { extra_argument } => {
                write!(
                    f,
                    "Too many positional arguments given. First unparsed argument was \"{extra_argument}\""
                )
            }
            Error::UnknownMode { mode } => {
                write!(f, "The mode \"{mode}\" is not valid")
            }
            Error::CannotDeletePart {
                part_id_name,
                constraining_parts,
            } => {
                write!(
                    f,
                    "Can't delete part with ID name \"{part_id_name}\" because the following {} group(s) reference it: [\"{}\"]",
                    constraining_parts.len(),
                    constraining_parts.join("\", \"")
                )
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

#[cfg(test)]
mod tests {
    use super::*;

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
            format!("{}", Error::FailedToSendSegmentRunEvent { index: 13 }),
            format!(
                "segment run event on segment #13 could not be sent because the receiver was closed",
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
                    display_name: Arc::from("display")
                }
            )
            .as_str(),
            "file name path given to segment or segment group \"display\" not absolute"
        );
        assert_eq!(
            format!(
                "{}",
                Error::EmptySegmentGroupInfo {
                    display_name: Arc::from("display")
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
                    display_name: Arc::from("dis\tplay")
                }
            )
            .as_str(),
            "the display name \"dis\tplay\" is invalid because it contains a tab character"
        );
        assert_eq!(
            format!(
                "{}",
                Error::IdNameInvalid {
                    id_name: Arc::from("i:d"),
                    component_separator: ":",
                }
            )
            .as_str(),
            "the ID name \"i:d\" is invalid because it contains either a tab or the string used as a component separator for segment groups: \":\""
        );
        assert_eq!(
            format!(
                "{}",
                Error::IdNameNotUnique {
                    id_name: Arc::from("a")
                }
            )
            .as_str(),
            "\"a\" is the ID name of multiple segments and/or groups. ID Names must be unique!"
        );
        assert_eq!(
            format!(
                "{}",
                Error::PartsDontExist {
                    id_name: Arc::from("a"),
                    display_name: Arc::from("A"),
                    invalid_parts: Arc::from([Arc::from("b"), Arc::from("c")])
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
                    id_name: Arc::from("a")
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
                    option: Arc::from("--option")
                }
            )
            .as_str(),
            "option \"--option\" was provided twice"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionExpectedNoValues {
                    option: Arc::from("--option"),
                    values: Arc::from([Arc::from("value")])
                }
            )
            .as_str(),
            "option \"--option\" expected no values, but got 1: [\"value\"]"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionExpectedOneValue {
                    option: Arc::from("--option"),
                    values: Arc::from([])
                }
            )
            .as_str(),
            "option \"--option\" expected one value, but got 0: []"
        );
        assert_eq!(
            format!(
                "{}",
                Error::OptionNotFound {
                    option: Arc::from("--option")
                }
            )
            .as_str(),
            "option \"--option\" not found"
        );
        assert_eq!(
            format!("{}", Error::NoModeFound).as_str(),
            "No CLI mode was found. Pass it as positional argument"
        );
        assert_eq!(
            format!(
                "{}",
                Error::TooManyPositionalArguments {
                    extra_argument: Arc::from("something uniquelkafd")
                }
            )
            .as_str(),
            "Too many positional arguments given. First unparsed argument was \"something uniquelkafd\""
        );
        assert_eq!(
            format!(
                "{}",
                Error::UnknownMode {
                    mode: Arc::from("weird unknown mode")
                }
            )
            .as_str(),
            "The mode \"weird unknown mode\" is not valid"
        );
        assert_eq!(
            format!(
                "{}",
                Error::CannotDeletePart {
                    part_id_name: Arc::from("to_delete"),
                    constraining_parts: Arc::from([
                        Arc::from("contains_to_delete"),
                        Arc::from("contains_contains_to_delete")
                    ])
                }
            )
            .as_str(),
            "Can't delete part with ID name \"to_delete\" because the following 2 group(s) reference it: [\"contains_to_delete\", \"contains_contains_to_delete\"]"
        )
    }
}
