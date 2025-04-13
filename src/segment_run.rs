//! Module with structs representing individual timed runs or groups of runs.
use std::fmt::{self, Debug, Display};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::utils::load_n_tokens;

/// A specific instant in time via the number of milliseconds since the unix epoch
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
    pub fn accumulate(&mut self, next: &Self) -> Result<()> {
        if self.end.0 == next.start.0 {
            self.deaths += next.deaths;
            self.end = next.end;
            Ok(())
        } else {
            Err(Error::SegmentCombinationError)
        }
    }

    pub fn accumulate_or_start(
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
    pub fn save(&self, path: &Path) -> Result<()> {
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

/// Description of a segment or group that was just completed
pub struct SupplementedSegmentRun {
    /// nesting level of the part that was just completed (how many groups into the run this is)
    pub nesting: u8,
    /// the name of the run part (either segment or group) that was completed
    pub name: String,
    /// number of deaths and start and end times of the part that was just completed
    pub segment_run: SegmentRun,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assert_pattern, coerce_pattern};
    use std::env::temp_dir;
    use std::sync::mpsc;
    use std::thread;

    use crate::utils::TempFile;

    /// Tests that SegmentRunEvent values display as expected
    #[test]
    fn segment_run_event_display() {
        assert_eq!(format!("{}", SegmentRunEvent::Death).as_str(), "Death");
        assert_eq!(format!("{}", SegmentRunEvent::End).as_str(), "End");
    }

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
        assert_pattern!(
            MillisecondsSinceEpoch::duration(start, end),
            Err(Error::NegativeDuration { start: 10, end: 0 })
        );
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
        assert_pattern!(
            existing.accumulate(&clone),
            Err(Error::SegmentCombinationError)
        );
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

    /// Tests the error that is returned when the run_receiver is dropped while running run_consecutive
    #[test]
    fn run_one_segment_closed_run_receiver() {
        let (event_sender, event_receiver) = mpsc::channel();
        let (run_sender, run_receiver) = mpsc::channel();
        drop(run_receiver);
        let receive_events = thread::spawn(move || {
            assert_pattern!(
                SegmentRun::run_consecutive(1, event_receiver, run_sender),
                Err(Error::FailedToSendSegment { index: 1 })
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
            let sub_error = coerce_pattern!(
                SegmentRun::run_consecutive(1, event_receiver, run_sender),
                Err(Error::ErrorDuringSegment {
                    index: 1,
                    error: sub_error
                }),
                sub_error
            );
            assert_pattern!(sub_error.as_ref(), Error::ChannelClosedBeforeEnd);
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

    /// Tests that load_all fails to load segment runs if there are too many or too few comas on a row
    #[test]
    fn load_all_segment_runs_not_enough_commas() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_not_enough_commas.csv"),
            "start,end,deaths\n0,10\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path),
            Err(Error::InvalidSegmentRunFileLine {
                index: 1,
                too_many_separators: false,
                expected_number_of_elements: 3
            })
        );
    }

    /// Tests that load_all fails to load segment runs if there are too many or too few comas on a row
    #[test]
    fn load_all_segment_runs_too_many_commas() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_too_many_commas.csv"),
            "start,end,deaths\n0,10,10,\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path),
            Err(Error::InvalidSegmentRunFileLine {
                index: 1,
                too_many_separators: true,
                expected_number_of_elements: 3
            })
        );
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
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path),
            Err(Error::SegmentRunFileLineParseError {
                index: 2,
                column: 0
            })
        );
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_1.csv"),
            "start,end,deaths\n0,1h,0\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path),
            Err(Error::SegmentRunFileLineParseError {
                index: 1,
                column: 1
            })
        );
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_2.csv"),
            "start,end,deaths\n0,10,0.\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path),
            Err(Error::SegmentRunFileLineParseError {
                index: 1,
                column: 2
            })
        );
    }

    /// Tests that the CouldNotReadSegmentRunFile error
    /// is returned if a non-existent file is passed
    #[test]
    fn load_all_segment_runs_nonexistent_file() {
        let temp =
            temp_dir().join("load_all_segment_runs_nonexistent_file.csv");
        let (path, error) = coerce_pattern!(
            SegmentRun::load_all(&temp),
            Err(Error::CouldNotReadSegmentRunFile { path, error }),
            (path, error)
        );
        assert_eq!(temp, path);
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
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

    /// Tests that the RunPart::save function can both create new files and append existing ones
    #[test]
    fn save_run() {
        let path = PathBuf::from(temp_dir().join("test_run.csv"));
        drop(TempFile { path: path.clone() });
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
}
