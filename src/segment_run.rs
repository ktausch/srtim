//! Module with structs representing individual timed runs or groups of runs.
use std::fmt::{self, Debug, Display};
use std::fs::{self, File};
use std::io::{self, Write};
use std::ops::{Div, Rem};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    mpsc::{Receiver, Sender},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};
use crate::utils::{IteratorWithMemory, load_n_tokens};

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

/// Finds the quotient and remainder when dividing numerator by denominator.
///
/// This function panics if denominator is zero!
fn divide_with_remainder<I>(numerator: I, denominator: I) -> (I, I)
where
    I: Div<Output = I> + Rem<Output = I> + Copy,
{
    (numerator / denominator, numerator % denominator)
}

/// Description of how an object should be printed
enum PrintStrategy {
    /// Nothing is printed
    DoNotShow,
    /// Prints with a fill character if the object is too short
    LeftFill {
        /// string literal printed after the (possibly padded) object
        after: &'static str,
        /// the character used to fill on the left
        fill: char,
        /// fill character is used until this width is reached
        width: usize,
    },
    /// prints the object directly with no filling
    NoFill {
        /// string literal printed after the object
        after: &'static str,
    },
}

impl PrintStrategy {
    /// Prints the object to the buffer based on this strategy
    fn print<T>(&self, object: T, buffer: &mut String)
    where
        T: Display,
    {
        match self {
            &PrintStrategy::DoNotShow => {}
            &PrintStrategy::LeftFill { fill, width, after } => {
                let object = format!("{object}");
                for _ in object.len()..width {
                    buffer.push(fill);
                }
                buffer.push_str(&object);
                if !after.is_empty() {
                    buffer.push_str(after);
                }
            }
            &PrintStrategy::NoFill { after } => {
                buffer.push_str(format!("{object}").as_str());
                if !after.is_empty() {
                    buffer.push_str(after);
                }
            }
        }
    }
}

/// Formats a duration so in the first of the following formats that is valid:
/// - S.MMM s
/// - M:SS.MMM
/// - H:MM:SS.MMM
pub fn format_duration(duration: Duration) -> String {
    let running = duration.as_millis();
    let (running, milliseconds) = divide_with_remainder(running, 1000);
    let (running, seconds) = divide_with_remainder(running, 60);
    let (hours, minutes) = divide_with_remainder(running, 60);
    let (ms_ps, s_ps, min_ps, h_ps) = if hours == 0 {
        let h_ps = PrintStrategy::DoNotShow;
        let (ms_ps, s_ps, min_ps) = if minutes == 0 {
            (
                PrintStrategy::LeftFill {
                    after: " s",
                    fill: '0',
                    width: 3,
                },
                PrintStrategy::NoFill { after: "." },
                PrintStrategy::DoNotShow,
            )
        } else {
            (
                PrintStrategy::LeftFill {
                    after: "",
                    fill: '0',
                    width: 3,
                },
                PrintStrategy::LeftFill {
                    after: ".",
                    fill: '0',
                    width: 2,
                },
                PrintStrategy::NoFill { after: ":" },
            )
        };
        (ms_ps, s_ps, min_ps, h_ps)
    } else {
        (
            PrintStrategy::LeftFill {
                after: "",
                fill: '0',
                width: 3,
            },
            PrintStrategy::LeftFill {
                after: ".",
                fill: '0',
                width: 2,
            },
            PrintStrategy::LeftFill {
                after: ":",
                fill: '0',
                width: 2,
            },
            PrintStrategy::NoFill { after: ":" },
        )
    };
    let mut result = String::with_capacity(15);
    h_ps.print(hours, &mut result);
    min_ps.print(minutes, &mut result);
    s_ps.print(seconds, &mut result);
    ms_ps.print(milliseconds, &mut result);
    result
}

/// An event that can take place during a segment
#[derive(Debug, Clone, Copy)]
pub enum SegmentRunEvent {
    /// death, used in speedrunning or challenge running
    Death,
    /// end of a segment
    End,
    /// cancel the entire run
    Cancel,
}

impl Display for SegmentRunEvent {
    /// Display the name of the enum
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

#[derive(Clone, Copy)]
pub struct Interval {
    /// the first instant in the interval
    pub start: MillisecondsSinceEpoch,
    /// the last instant in the interval
    pub end: MillisecondsSinceEpoch,
}

impl Interval {
    /// The amount of time an interval represents as a Duration object
    pub fn duration(&self) -> Duration {
        Duration::from_millis((self.end.0 - self.start.0).try_into().expect(
            "The program cannot handle durations of 2^64 or more milliseconds",
        ))
    }

    /// True if this interval completely envelopes the other interval
    pub fn contains(&self, other: &Self) -> bool {
        (self.start.0 <= other.start.0) && (self.end.0 >= other.end.0)
    }
}

/// Information about a single segment run
#[derive(Clone, Copy)]
pub struct SegmentRun {
    /// the number of deaths in the run
    pub deaths: u32,
    /// the time at which the run occurred
    pub interval: Interval,
    /// true if this run was canceled before it was completed
    pub canceled: bool,
}

impl SegmentRun {
    /// Adds the given run to this one. If next.interval.start does not
    /// match self.interval.end, a SegmentCombinationError is returned
    pub fn accumulate(&mut self, next: &Self) -> Result<()> {
        if self.interval.end.0 == next.interval.start.0 {
            self.deaths += next.deaths;
            self.interval.end = next.interval.end;
            self.canceled = self.canceled || next.canceled;
            Ok(())
        } else {
            Err(Error::SegmentCombinationError)
        }
    }

    /// Returns a new SegmentRun equal to next if maybe_started is None.
    /// Accumulates next onto the SegmentRun if maybe_started is Some.
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
        let mut canceled = false;
        loop {
            match receiver.recv() {
                Ok(SegmentRunEvent::End) => {
                    break;
                }
                Ok(SegmentRunEvent::Death) => {
                    deaths += 1;
                }
                Ok(SegmentRunEvent::Cancel) => {
                    canceled = true;
                    break;
                }
                Err(_) => {
                    return Err(Error::ChannelClosedBeforeEnd);
                }
            };
        }
        let end = MillisecondsSinceEpoch::now();
        Ok(SegmentRun {
            deaths,
            interval: Interval { start, end },
            canceled,
        })
    }

    /// The amount of time a run took as a Duration object
    pub fn duration(&self) -> Duration {
        self.interval.duration()
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
                    start = segment_run.interval.end;
                    let canceled = segment_run.canceled;
                    match segment_run_sender.send(segment_run) {
                        Ok(()) => {
                            if canceled {
                                return Ok(());
                            }
                        }
                        Err(_) => {
                            return Err(Error::FailedToSendSegment { index });
                        }
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
                    file.write(b"start,end,deaths,canceled\n")
                        .map_err(io_error_map)?;
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
            format!(
                "{},{},{},{}\n",
                self.interval.start.0,
                self.interval.end.0,
                self.deaths,
                if self.canceled { '1' } else { '0' }
            )
            .as_bytes(),
        )
        .map_err(io_error_map)?;
        Ok(())
    }

    /// Loads all SegmentRun objects stored in a csv file.
    pub fn load_all(
        path: &Path,
        include_canceled: bool,
    ) -> Result<Arc<[SegmentRun]>> {
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
        let expected_number_of_elements = 4 as usize;
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
            let canceled = tokens[3].parse::<u8>().map_err(|_| {
                Error::SegmentRunFileLineParseError { index, column: 3 }
            })?;
            let canceled = canceled != 0;
            if canceled && !include_canceled {
                continue;
            }
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
            result.push(SegmentRun {
                interval: Interval { start, end },
                deaths,
                canceled,
            });
        }
        Ok(result.into())
    }
}

/// Description of a segment or group that was just completed
pub struct SupplementedSegmentRun {
    /// nesting level of the part that was just completed (how many groups into the run this is)
    pub nesting: u8,
    /// the name of the run part (either segment or group) that was completed
    pub name: Arc<str>,
    /// number of deaths and start and end times of the part that was just completed
    pub segment_run: SegmentRun,
}

/// A struct containing basic stats (min, max, mean, median) for a set of values
pub struct BasicStats<T, M> {
    /// the best run
    pub best: T,
    /// the worst run
    pub worst: T,
    /// the average of all of the runs
    pub mean: M,
    /// the median of all of the runs
    pub median: M,
}

/// Contains basic stats for both duration and death count.
pub struct SegmentStats {
    /// the number of runs that went into making the stats
    pub num_runs: u32,
    /// the min, max, mean, and median duration
    pub durations: BasicStats<Duration, Duration>,
    /// the min, max, mean, and median number of deaths
    pub deaths: BasicStats<u32, f32>,
}

impl SegmentStats {
    /// Computes the min, max, mean, and median of the duration
    /// and death count of all runs in the slice.
    pub fn from_runs<'a>(
        runs: impl IntoIterator<Item = &'a SegmentRun>,
    ) -> Option<SegmentStats> {
        let mut runs = runs.into_iter();
        let run = runs.next()?;
        let duration = run.duration();
        let mut sum_deaths = run.deaths;
        let mut sum_duration = duration.as_millis();
        let mut durations = Vec::new();
        let mut deaths = Vec::new();
        let mut num_runs = 1;
        durations.push(sum_duration);
        deaths.push(sum_deaths);
        while let Some(run) = runs.next() {
            num_runs += 1;
            let milliseconds = run.duration().as_millis();
            sum_duration += milliseconds;
            sum_deaths += run.deaths;
            durations.push(milliseconds);
            deaths.push(run.deaths);
        }
        durations.sort_unstable();
        deaths.sort_unstable();
        Some(SegmentStats {
            num_runs: (num_runs as u32),
            durations: BasicStats {
                best: Duration::from_millis(
                    (*durations.first().unwrap()).try_into().unwrap(),
                ),
                worst: Duration::from_millis(
                    (*durations.last().unwrap()).try_into().unwrap(),
                ),
                mean: Duration::from_millis(
                    (sum_duration / (num_runs as u128)).try_into().unwrap(),
                ),
                median: Duration::from_millis(
                    ((durations[durations.len() / 2]
                        + durations[(durations.len() - 1) / 2])
                        / 2)
                    .try_into()
                    .unwrap(),
                ),
            },
            deaths: BasicStats {
                best: *deaths.first().unwrap(),
                worst: *deaths.last().unwrap(),
                mean: (sum_deaths as f32) / (num_runs as f32),
                median: (((deaths[deaths.len() / 2]
                    + deaths[(deaths.len() - 1) / 2])
                    as f32)
                    / (2 as f32)),
            },
        })
    }
}

/// Collects all runs that are both during a segment in the during list
/// and not during a segment in the not_during list.
pub fn filter_runs<'a, R, D, N>(
    mut runs: R,
    during: Vec<D>,
    not_during: Vec<N>,
) -> Vec<&'a SegmentRun>
where
    D: Iterator<Item = &'a SegmentRun>,
    N: Iterator<Item = &'a SegmentRun>,
    R: Iterator<Item = &'a SegmentRun>,
{
    let mut during_iters: Vec<_> = during
        .into_iter()
        .map(|iterator| IteratorWithMemory::new(iterator))
        .collect();
    let mut not_during_iters: Vec<_> = not_during
        .into_iter()
        .map(|iterator| IteratorWithMemory::new(iterator))
        .collect();
    let mut result = Vec::new();
    'outer: while let Some(run) = runs.next() {
        let run_end = run.interval.end.0;
        for during in during_iters.iter_mut() {
            during.advance_until(|&during_run| {
                during_run.interval.end.0 >= run_end
            });
            match during.current() {
                Some(&during_run) => {
                    if during_run.interval.start.0 > run.interval.start.0 {
                        continue 'outer;
                    }
                }
                None => {
                    continue 'outer;
                }
            }
        }
        for not_during in not_during_iters.iter_mut() {
            not_during.advance_until(|&not_during_run| {
                not_during_run.interval.end.0 >= run_end
            });
            if let Some(&not_during_run) = not_during.current() {
                if not_during_run.interval.start.0 <= run.interval.start.0 {
                    continue 'outer;
                }
            }
        }
        result.push(run);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{assert_pattern, coerce_pattern};
    use std::env::temp_dir;
    use std::ptr;
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
            interval: Interval {
                start: MillisecondsSinceEpoch(0),
                end: MillisecondsSinceEpoch(10),
            },
            canceled: false,
        };
        let new = SegmentRun {
            deaths: 2,
            interval: Interval {
                start: MillisecondsSinceEpoch(10),
                end: MillisecondsSinceEpoch(30),
            },
            canceled: false,
        };

        accumulator.accumulate(&new).unwrap();
        assert_eq!(accumulator.deaths, 3);
        assert_eq!(accumulator.interval.start.0, 0);
        assert_eq!(accumulator.interval.end.0, 30);
        assert_eq!(accumulator.duration(), Duration::from_millis(30));
    }

    /// Tests that an error is returned when attempting to combine segment runs that have time between them
    #[test]
    fn combine_segment_runs_no_shared_endpoint() {
        let mut existing = SegmentRun {
            deaths: 1,
            interval: Interval {
                start: MillisecondsSinceEpoch(0),
                end: MillisecondsSinceEpoch(10),
            },
            canceled: false,
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
            interval: Interval {
                start: MillisecondsSinceEpoch(0),
                end: MillisecondsSinceEpoch(500_000),
            },
            canceled: false,
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
            "start,end,deaths,canceled\n0,10,0\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path, true),
            Err(Error::InvalidSegmentRunFileLine {
                index: 1,
                too_many_separators: false,
                expected_number_of_elements: 4
            })
        );
    }

    /// Tests that load_all fails to load segment runs if there are too many or too few comas on a row
    #[test]
    fn load_all_segment_runs_too_many_commas() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_too_many_commas.csv"),
            "start,end,deaths,canceled\n0,10,10,0,\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path, true),
            Err(Error::InvalidSegmentRunFileLine {
                index: 1,
                too_many_separators: true,
                expected_number_of_elements: 4
            })
        );
    }

    /// Tests that a SegmentRunFileLineParseError is returned if
    /// a column has a token that isn't parsable as an integer.
    #[test]
    fn load_all_segment_runs_unparsable() {
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_0.csv"),
            "start,end,deaths,canceled\n0,10,0,0\n15.,20,1,0\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path, true),
            Err(Error::SegmentRunFileLineParseError {
                index: 2,
                column: 0
            })
        );
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_1.csv"),
            "start,end,deaths,canceled\n0,1h,0,0\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path, true),
            Err(Error::SegmentRunFileLineParseError {
                index: 1,
                column: 1
            })
        );
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_2.csv"),
            "start,end,deaths,canceled\n0,10,0.,1\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path, true),
            Err(Error::SegmentRunFileLineParseError {
                index: 1,
                column: 2
            })
        );
        let temp_file = TempFile::with_contents(
            temp_dir().join("load_all_segment_runs_unparsable_3.csv"),
            "start,end,deaths,canceled\n0,10,0,yes\n",
        )
        .unwrap();
        assert_pattern!(
            SegmentRun::load_all(&temp_file.path, true),
            Err(Error::SegmentRunFileLineParseError {
                index: 1,
                column: 3
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
            SegmentRun::load_all(&temp, true),
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
            "start,end,deaths,canceled\n0,100,10,0\n1000,2000,2,1\n",
        )
        .unwrap();
        let segment_runs = SegmentRun::load_all(&temp_file.path, true).unwrap();
        assert_eq!(segment_runs.len(), 2);
        let segment_run = segment_runs[0];
        assert_eq!(segment_run.deaths, 10);
        assert_eq!(segment_run.duration(), Duration::from_millis(100));
        assert!(!segment_run.canceled);
        let segment_run = segment_runs[1];
        assert_eq!(segment_run.deaths, 2);
        assert_eq!(segment_run.duration(), Duration::from_millis(1000));
        assert!(segment_run.canceled);
        let segment_runs =
            SegmentRun::load_all(&temp_file.path, false).unwrap();
        assert_eq!(segment_runs.len(), 1);
        let segment_run = segment_runs[0];
        assert_eq!(segment_run.deaths, 10);
        assert_eq!(segment_run.duration(), Duration::from_millis(100));
        assert!(!segment_run.canceled);
    }

    /// Tests that the RunPart::save function can both create new files and append existing ones
    #[test]
    fn save_run() {
        let path = PathBuf::from(temp_dir().join("test_run.csv"));
        drop(TempFile { path: path.clone() });
        let first = SegmentRun {
            deaths: 3,
            interval: Interval {
                start: MillisecondsSinceEpoch(1000),
                end: MillisecondsSinceEpoch(91000),
            },
            canceled: false,
        };
        first.save(&path).unwrap();
        assert_eq!(
            fs::read_to_string(path.clone()).unwrap().as_str(),
            "start,end,deaths,canceled\n1000,91000,3,0\n"
        );
        let second = SegmentRun {
            deaths: 1,
            interval: Interval {
                start: MillisecondsSinceEpoch(100000),
                end: MillisecondsSinceEpoch(200000),
            },
            canceled: true,
        };
        second.save(&path).unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap().as_str(),
            "start,end,deaths,canceled\n1000,91000,3,0\n100000,200000,1,1\n"
        );
    }

    /// Creates and fills a string using the given PrintStrategy
    /// to make a string of the given object.
    fn print_owned<T>(print_strategy: &PrintStrategy, object: T) -> String
    where
        T: Display,
    {
        let mut buffer = String::new();
        print_strategy.print(object, &mut buffer);
        buffer
    }

    /// Ensures that the DoNotShow strategy causes nothing to be written.
    #[test]
    fn test_print_strategy_do_not_show() {
        let do_not_show = PrintStrategy::DoNotShow;
        assert!(print_owned(&do_not_show, "a").is_empty());
        assert!(print_owned(&do_not_show, 1).is_empty());
        assert!(print_owned(&do_not_show, String::from("b")).is_empty());
    }

    /// Ensures that the left fill PrintStrategy correctly pads with
    /// fill characters for short-string objects and does no padding
    /// for long-string objects. Also ensures that the "after" string
    /// literal is printed correctly at the end.
    #[test]
    fn test_print_srategy_left_fill() {
        let left_fill = PrintStrategy::LeftFill {
            after: "hello!",
            fill: '0',
            width: 4,
        };
        assert_eq!(print_owned(&left_fill, 1).as_str(), "0001hello!");
        assert_eq!(print_owned(&left_fill, "abcde").as_str(), "abcdehello!");
        let left_fill = PrintStrategy::LeftFill {
            after: "",
            fill: '-',
            width: 4,
        };
        assert_eq!(print_owned(&left_fill, "bed").as_str(), "-bed");
        assert_eq!(print_owned(&left_fill, "").as_str(), "----");
    }

    /// Ensures that the no fill PrintStrategy directly prints
    /// the object and the "after" string literal.
    #[test]
    fn test_print_strategy_no_fill() {
        let no_fill = PrintStrategy::NoFill {
            after: "totallyrandom",
        };
        assert_eq!(print_owned(&no_fill, 1001).as_str(), "1001totallyrandom");
        assert_eq!(print_owned(&no_fill, "a").as_str(), "atotallyrandom");
        let no_fill = PrintStrategy::NoFill { after: "" };
        assert_eq!(print_owned(&no_fill, 1001).as_str(), "1001");
        assert_eq!(print_owned(&no_fill, "a").as_str(), "a");
    }

    /// Ensures that divide_with_remainder panics if passed a zero denominator.
    #[test]
    #[should_panic]
    fn test_divide_with_remainder_zero_denominator() {
        divide_with_remainder(1, 0);
    }

    /// Tests the divide_with_remainder function with various integer types.
    #[test]
    fn test_divide_with_remainder() {
        assert_eq!(divide_with_remainder(0usize, 1usize), (0, 0));
        assert_eq!(divide_with_remainder(648, 7), (92, 4));
        assert_eq!(divide_with_remainder(255u8, 128u8), (1, 127));
    }

    /// Tests all different forms of Duration formatting used in the program.
    #[test]
    fn test_format_duration() {
        assert_eq!(
            format_duration(Duration::from_millis(3724127)).as_str(),
            "1:02:04.127"
        );
        assert_eq!(
            format_duration(Duration::from_millis(3600000000)).as_str(),
            "1000:00:00.000"
        );
        assert_eq!(
            format_duration(Duration::from_millis(708846)).as_str(),
            "11:48.846"
        );
        assert_eq!(
            format_duration(Duration::from_millis(43762)).as_str(),
            "43.762 s"
        );
        assert_eq!(
            format_duration(Duration::from_millis(5683)).as_str(),
            "5.683 s"
        );
        assert_eq!(
            format_duration(Duration::from_millis(74)).as_str(),
            "0.074 s"
        );
    }

    /// Tests that the SegmentStats::from_runs method
    /// returns None if there are no runs.
    #[test]
    fn test_stats_zero_runs() {
        let runs: Arc<[SegmentRun]> = Arc::from([]);
        assert!(SegmentStats::from_runs(runs.into_iter()).is_none());
    }

    /// Tests that the SegmentStats::from_runs method has the best, worst,
    /// mean, and median equal to the single run if there is only one.
    #[test]
    fn test_stats_single_run() {
        let runs: Arc<[SegmentRun]> = Arc::new([SegmentRun {
            deaths: 7,
            interval: Interval {
                start: MillisecondsSinceEpoch(0),
                end: MillisecondsSinceEpoch(3624000),
            },
            canceled: false,
        }]);
        let duration = Duration::from_secs(3624);
        let stats = SegmentStats::from_runs(runs.into_iter()).unwrap();
        assert_eq!(stats.num_runs, 1);
        assert_eq!(duration, stats.durations.best);
        assert_eq!(duration, stats.durations.worst);
        assert_eq!(duration, stats.durations.mean);
        assert_eq!(duration, stats.durations.median);
        assert_eq!(7u32, stats.deaths.best);
        assert_eq!(7u32, stats.deaths.worst);
        assert_eq!(7f32, stats.deaths.mean);
        assert_eq!(7f32, stats.deaths.median);
    }

    /// Tests that the SegmentStats::from_runs method has the best, worst,
    /// mean, and median equal to the expected values when there are two runs.
    #[test]
    fn test_stats_two_runs() {
        let runs: Arc<[SegmentRun]> = Arc::from([
            SegmentRun {
                deaths: 7,
                interval: Interval {
                    start: MillisecondsSinceEpoch(0),
                    end: MillisecondsSinceEpoch(3624000),
                },
                canceled: false,
            },
            SegmentRun {
                deaths: 6,
                interval: Interval {
                    start: MillisecondsSinceEpoch(0),
                    end: MillisecondsSinceEpoch(3600000),
                },
                canceled: false,
            },
        ]);
        let stats = SegmentStats::from_runs(runs.into_iter()).unwrap();
        assert_eq!(stats.num_runs, 2);
        assert_eq!(Duration::from_secs(3600), stats.durations.best);
        assert_eq!(Duration::from_secs(3624), stats.durations.worst);
        assert_eq!(Duration::from_secs(3612), stats.durations.mean);
        assert_eq!(Duration::from_secs(3612), stats.durations.median);
        assert_eq!(6u32, stats.deaths.best);
        assert_eq!(7u32, stats.deaths.worst);
        assert_eq!(6.5f32, stats.deaths.mean);
        assert_eq!(6.5f32, stats.deaths.median);
    }

    /// Tests that the SegmentStats::from_runs method
    /// has the best, worst, mean, and median equal to
    /// the expected values when there are three runs.
    #[test]
    fn test_stats_three_runs() {
        let runs: Arc<[SegmentRun]> = Arc::from([
            SegmentRun {
                deaths: 1,
                interval: Interval {
                    start: MillisecondsSinceEpoch(0),
                    end: MillisecondsSinceEpoch(20000),
                },
                canceled: false,
            },
            SegmentRun {
                deaths: 2,
                interval: Interval {
                    start: MillisecondsSinceEpoch(0),
                    end: MillisecondsSinceEpoch(30000),
                },
                canceled: false,
            },
            SegmentRun {
                deaths: 6,
                interval: Interval {
                    start: MillisecondsSinceEpoch(0),
                    end: MillisecondsSinceEpoch(100000),
                },
                canceled: false,
            },
        ]);
        let stats = SegmentStats::from_runs(runs.into_iter()).unwrap();
        assert_eq!(stats.num_runs, 3);
        assert_eq!(Duration::from_secs(20), stats.durations.best);
        assert_eq!(Duration::from_secs(100), stats.durations.worst);
        assert_eq!(Duration::from_secs(50), stats.durations.mean);
        assert_eq!(Duration::from_secs(30), stats.durations.median);
        assert_eq!(1u32, stats.deaths.best);
        assert_eq!(6u32, stats.deaths.worst);
        assert_eq!(3f32, stats.deaths.mean);
        assert_eq!(2f32, stats.deaths.median);
    }

    /// Tests the SegmentRun::run associated function with a single death and segment end.
    #[test]
    fn run_single_happy_path() {
        let start = MillisecondsSinceEpoch::now();
        let (event_sender, event_receiver) = mpsc::channel();
        let input_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::End).unwrap();
        });
        let run = SegmentRun::run(&event_receiver, start).unwrap();
        input_thread.join().unwrap();
        assert_eq!(run.deaths, 1);
        assert!(run.duration().as_millis() >= 9);
        assert!(!run.canceled);
    }

    /// Tests the SegmentRun::run associated function
    /// with a single death and then a cancellation.
    #[test]
    fn run_single_canceled() {
        let start = MillisecondsSinceEpoch::now();
        let (event_sender, event_receiver) = mpsc::channel();
        let input_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::Cancel).unwrap();
        });
        let run = SegmentRun::run(&event_receiver, start).unwrap();
        input_thread.join().unwrap();
        assert_eq!(run.deaths, 2);
        assert!(run.duration().as_millis() >= 9);
        assert!(run.canceled);
    }

    /// Tests the SegmentRun::run_consecutive associated function
    /// with a three segment run with no cancellations.
    #[test]
    fn run_multiple_happy_path() {
        let (event_sender, event_receiver) = mpsc::channel();
        let (segment_run_sender, segment_run_receiver) = mpsc::channel();
        let input_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::End).unwrap();
        });
        let output_thread = thread::spawn(move || {
            let mut runs = Vec::new();
            while let Ok(run) = segment_run_receiver.recv() {
                runs.push(run);
            }
            runs
        });
        SegmentRun::run_consecutive(3, event_receiver, segment_run_sender)
            .unwrap();
        input_thread.join().unwrap();
        let runs = output_thread.join().unwrap();
        assert_eq!(runs.len(), 3);
        let (first_run, second_run, third_run) = (runs[0], runs[1], runs[2]);
        assert_eq!(first_run.deaths, 2);
        assert!(first_run.duration().as_millis() >= 9);
        assert!(!first_run.canceled);
        assert_eq!(second_run.deaths, 1);
        assert!(second_run.duration().as_millis() >= 9);
        assert!(!second_run.canceled);
        assert_eq!(third_run.deaths, 0);
        assert!(third_run.duration().as_millis() >= 9);
        assert!(!third_run.canceled);
    }

    /// Tests the SegmentRun::run_consecutive associated function with
    /// a three segment run that is canceled during the second segment.
    #[test]
    fn run_multiple_canceled() {
        let (event_sender, event_receiver) = mpsc::channel();
        let (segment_run_sender, segment_run_receiver) = mpsc::channel();
        let input_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(10));
            event_sender.send(SegmentRunEvent::Death).unwrap();
            event_sender.send(SegmentRunEvent::Cancel).unwrap();
        });
        let output_thread = thread::spawn(move || {
            let mut runs = Vec::new();
            while let Ok(run) = segment_run_receiver.recv() {
                runs.push(run);
            }
            runs
        });
        SegmentRun::run_consecutive(3, event_receiver, segment_run_sender)
            .unwrap();
        input_thread.join().unwrap();
        let runs = output_thread.join().unwrap();
        assert_eq!(runs.len(), 2);
        let (first_run, second_run) = (runs[0], runs[1]);
        assert_eq!(first_run.deaths, 2);
        assert!(first_run.duration().as_millis() >= 9);
        assert!(!first_run.canceled);
        assert_eq!(second_run.deaths, 1);
        assert!(second_run.duration().as_millis() >= 9);
        assert!(second_run.canceled);
    }

    /// Tests that the filter_runs function acts same as .collect()
    /// on runs if there are no during or not_during
    #[test]
    fn filter_runs_no_filters() {
        let first = SegmentRun {
            deaths: 1,
            interval: Interval {
                start: MillisecondsSinceEpoch(0),
                end: MillisecondsSinceEpoch(100),
            },
            canceled: true,
        };
        let second = SegmentRun {
            deaths: 0,
            interval: Interval {
                start: MillisecondsSinceEpoch(1000),
                end: MillisecondsSinceEpoch(1200),
            },
            canceled: false,
        };
        let third = SegmentRun {
            deaths: 2,
            interval: Interval {
                start: MillisecondsSinceEpoch(100_000),
                end: MillisecondsSinceEpoch(101_000),
            },
            canceled: false,
        };
        let during: Vec<std::vec::IntoIter<&SegmentRun>> = Vec::new();
        let not_during: Vec<std::vec::IntoIter<&SegmentRun>> = Vec::new();
        let mut filtered = filter_runs(
            vec![&first, &second, &third].into_iter(),
            during,
            not_during,
        )
        .into_iter();
        assert!(ptr::eq(filtered.next().unwrap(), &first));
        assert!(ptr::eq(filtered.next().unwrap(), &second));
        assert!(ptr::eq(filtered.next().unwrap(), &third));
        assert!(filtered.next().is_none());
    }

    /// Tests that the filter_runs function correctly accounts for
    /// during and not_during when some iterators are passed to them.
    #[test]
    fn filter_runs_with_filters() {
        let first = SegmentRun {
            deaths: 1,
            interval: Interval {
                start: MillisecondsSinceEpoch(0),
                end: MillisecondsSinceEpoch(100),
            },
            canceled: true,
        };
        let second = SegmentRun {
            deaths: 0,
            interval: Interval {
                start: MillisecondsSinceEpoch(1000),
                end: MillisecondsSinceEpoch(1200),
            },
            canceled: false,
        };
        let third = SegmentRun {
            deaths: 2,
            interval: Interval {
                start: MillisecondsSinceEpoch(100_000),
                end: MillisecondsSinceEpoch(101_000),
            },
            canceled: false,
        };
        let fourth = SegmentRun {
            deaths: 4,
            interval: Interval {
                start: MillisecondsSinceEpoch(1_000),
                end: MillisecondsSinceEpoch(200_000),
            },
            canceled: false,
        };
        let fifth = SegmentRun {
            deaths: 3,
            interval: Interval {
                start: MillisecondsSinceEpoch(100_000),
                end: MillisecondsSinceEpoch(150_000),
            },
            canceled: false,
        };
        let during = vec![vec![&fourth].into_iter()];
        let not_during = vec![vec![&fifth].into_iter()];
        let mut filtered = filter_runs(
            vec![&first, &second, &third].into_iter(),
            during,
            not_during,
        )
        .into_iter();
        assert!(ptr::eq(filtered.next().unwrap(), &second));
        assert!(filtered.next().is_none());
    }
}
