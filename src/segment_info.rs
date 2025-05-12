//! Module with
//! 1. owning-types defining segments and segment groups
//! 2. non-owning-type defining a part of a run in a tree like structure
use std::collections::{HashMap, hash_map::Entry};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::coerce_pattern;
use crate::error::{Error, Result};
use crate::segment_run::{SegmentRun, SegmentRunEvent, SupplementedSegmentRun};
use crate::utils::zip_same;

/// Ensures that display_name doesn't have a tab,
/// returning a DisplayNameInvalid if it does
fn check_display_name(display_name: &str) -> Result<()> {
    if display_name.contains('\t') {
        Err(Error::DisplayNameInvalid {
            display_name: Arc::from(display_name),
        })
    } else {
        Ok(())
    }
}

/// Logistical information about a single segment, including the name that
/// should be displayed and the file in which its information is stored.
pub struct SegmentInfo {
    /// name that should be displayed for this segment
    pub display_name: Arc<str>,
    /// absolute path to file containing this segment's info
    pub file_name: PathBuf,
}

impl SegmentInfo {
    /// Creates a new SegmentInfo, ensuring that file_name is an absolute path
    pub fn new(
        display_name: Arc<str>,
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
pub struct SegmentGroupInfo {
    /// name that should be displayed for this group
    pub display_name: Arc<str>,
    /// non-empty list of ID names of the parts that make up this group
    pub part_id_names: Arc<[Arc<str>]>,
    /// absolute path to file containing information about this group's runs
    pub file_name: PathBuf,
}

impl SegmentGroupInfo {
    /// Creates a new SegmentGroupInfo, ensuring file_name is an absolute path and part_id_names is non-empty
    pub fn new(
        display_name: Arc<str>,
        part_id_names: Arc<[Arc<str>]>,
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
pub enum RunPart<'a> {
    /// part of a run that is just a single segment
    SingleSegment(&'a SegmentInfo),
    /// part of a run that is a (possibly nested) group of segments
    Group {
        components: Arc<[&'a RunPart<'a>]>,
        display_name: Arc<str>,
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
fn nested_segment_specs_from_parts_slice<'a>(
    components: &'a [&'a RunPart<'a>],
) -> Vec<Vec<(usize, &'a RunPart<'a>)>> {
    let mut result = Vec::new();
    for (index, component) in components.iter().enumerate() {
        match component {
            RunPart::Group { components, .. } => {
                let mut leaves =
                    nested_segment_specs_from_parts_slice(components)
                        .into_iter();
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

/// Owned display and path names for a given run part
pub struct PartCore {
    /// the human-readable name of the part
    pub display_name: Arc<str>,
    /// the path where the part's runs are stored
    pub file_name: PathBuf,
    /// which instance of this part it is in the part above it
    pub index: usize,
}

/// PartialEq needs to be implemented for PartCore for it to be used in ZipSame
impl PartialEq for PartCore {
    /// Since display names can be duplicated but paths cannot, == is just checks paths
    fn eq(&self, other: &Self) -> bool {
        (self.file_name == other.file_name) && (self.index == other.index)
    }
}

impl<'a> RunPart<'a> {
    /// Gets the display name of the part, whether it is a segment or a segment group
    pub fn display_name(&'a self) -> &'a Arc<str> {
        match self {
            RunPart::Group { display_name, .. } => display_name,
            RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                display_name
            }
        }
    }

    /// Gets the file name of the part, whether it is a segment or a segment group
    pub fn file_name(&'a self) -> &'a Path {
        match self {
            &RunPart::Group { file_name, .. } => file_name,
            &RunPart::SingleSegment(SegmentInfo { file_name, .. }) => {
                file_name.as_path()
            }
        }
    }

    /// Gets the specs for all low-level segments of the run (i.e. how they connect back to the full run)
    pub fn nested_segment_specs(
        &'a self,
    ) -> Vec<Vec<(usize, &'a RunPart<'a>)>> {
        match self {
            RunPart::Group { components, .. } => {
                nested_segment_specs_from_parts_slice(components)
            }
            _ => vec![vec![]],
        }
    }

    /// Gets the nested segment names (the single top-level name is always implied)
    /// when the run part is a group, None if the run part is a single segment.
    ///
    /// For example, if the full run part is (((AB)C)D) then the vectors will contain:
    /// [[((AB)C), (AB), A], [((AB)C), (AB), B], [((AB)C), C], [D]]
    pub fn nested_segment_names(&self) -> Vec<Vec<PartCore>> {
        self.nested_segment_specs()
            .into_iter()
            .map(|segment_spec| {
                segment_spec
                    .into_iter()
                    .map(|(index, part)| PartCore {
                        display_name: part.display_name().clone(),
                        file_name: PathBuf::from(part.file_name()),
                        index,
                    })
                    .collect()
            })
            .collect()
    }

    /// Collects references to all of the SegmentInfo objects of low-level segments into a vector
    #[allow(dead_code)]
    pub fn run_segments(&self) -> Vec<&SegmentInfo> {
        match self {
            &RunPart::Group { .. } => self
                .nested_segment_specs()
                .iter()
                .map(|s| {
                    coerce_pattern!(
                        s.last().unwrap().1,
                        &RunPart::SingleSegment(segment),
                        segment
                    )
                })
                .collect(),
            &RunPart::SingleSegment(segment) => vec![segment],
        }
    }

    /// Creates a string with one line per segment. Each segment is specified by its full
    /// branch, i.e. <group display name>, <group display name>, <segment display name>
    /// separated by tabs. However, if a group continues from one segment to the next, it
    /// is omitted and replaced by a number of spaces equal to the length of the name.
    pub fn tree(&'a self) -> String {
        let mut result = String::new();
        let mut segment_specs = self.nested_segment_specs().into_iter();
        let mut last_segment_spec: Vec<(usize, &'a RunPart<'a>)> = Vec::new();
        let mut first_iteration = true;
        loop {
            let segment_spec = match segment_specs.next() {
                None => break,
                Some(incomplete_segment_spec) => {
                    let mut segment_spec =
                        Vec::with_capacity(incomplete_segment_spec.len() + 1);
                    segment_spec.push((0, self));
                    for part in incomplete_segment_spec {
                        segment_spec.push(part);
                    }
                    segment_spec
                }
            };
            let equal_display_name_lengths: Vec<usize> = zip_same(
                last_segment_spec
                    .iter()
                    .map(|(index, part)| (index, part.display_name())),
                segment_spec
                    .iter()
                    .map(|(index, part)| (index, part.display_name())),
            )
            .map(|(_, display_name)| display_name.len())
            .collect();
            if first_iteration {
                first_iteration = false;
            } else {
                result.push('\n');
            }
            equal_display_name_lengths.iter().for_each(
                |&equal_display_name_length| {
                    for _ in 0..equal_display_name_length {
                        result.push(' ');
                    }
                    result.push('\t');
                },
            );
            for index in equal_display_name_lengths.len()..segment_spec.len() {
                result.push_str(segment_spec[index].1.display_name());
                if index + 1 != segment_spec.len() {
                    result.push_str("\t");
                }
            }
            last_segment_spec = segment_spec;
        }
        result
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
    ///     write: if true, then write to files. Otherwise, no writing to disk
    pub fn run(
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
        let num_segments = nested_segment_names.len();
        let top_level_name = self.display_name().clone();
        let top_level_path = PathBuf::from(self.file_name());
        let mut maybe_top_level_accumulation: Option<SegmentRun> = None;
        let tagging_thread = thread::spawn(move || {
            let mut nested_segment_names = nested_segment_names.into_iter();
            let mut last_segment_names = nested_segment_names.next().unwrap();
            let mut accumulations: HashMap<Arc<str>, SegmentRun> =
                HashMap::new();
            loop {
                let next_segment_names = match nested_segment_names.next() {
                    Some(next_segment_names) => next_segment_names,
                    None => vec![],
                };
                let mut nesting = last_segment_names.len() as u8;
                let segment_run: SegmentRun = segment_run_receiver
                    .recv()
                    .map_err(|_| Error::SegmentRunSenderClosed)?;
                maybe_top_level_accumulation =
                    Some(SegmentRun::accumulate_or_start(
                        maybe_top_level_accumulation,
                        &segment_run,
                    )?);
                let canceled = segment_run.canceled;
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
                while last_segment_names.len()
                    > (if canceled { 0 } else { num_continuing_parts })
                {
                    let PartCore {
                        display_name: last_segment_name,
                        file_name: last_segment_path,
                        ..
                    } = last_segment_names.pop().unwrap();
                    let full_segment_run = match accumulations
                        .remove_entry(&last_segment_name)
                    {
                        Some((_, mut full_segment_run)) => {
                            if last_segment_names.len() >= num_continuing_parts
                            {
                                full_segment_run.accumulate(&segment_run)?;
                            }
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
                if canceled {
                    break;
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

    /// Gets all runs of the given run part
    pub fn get_runs(
        &self,
        include_canceled: bool,
    ) -> Result<Arc<[SegmentRun]>> {
        match SegmentRun::load_all(self.file_name(), include_canceled) {
            Ok(runs) => Ok(runs),
            Err(Error::CouldNotReadSegmentRunFile { path, error }) => {
                if error.kind() == ErrorKind::NotFound {
                    Ok(Arc::new([]))
                } else {
                    Err(Error::CouldNotReadSegmentRunFile { path, error })
                }
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use std::time::Duration;

    use crate::coerce_pattern;

    /// Ensures that SegmentInfo requires an absolute path file_name
    #[test]
    fn segment_info_no_absolute_path() {
        assert!(
            SegmentInfo::new(Arc::from("A"), PathBuf::from("A.txt")).is_err()
        );
    }

    /// Ensures that SegmentGroupInfo requires an absolute path file_name
    #[test]
    fn segment_group_info_no_absolute_path() {
        assert!(
            SegmentGroupInfo::new(
                Arc::from("A"),
                Arc::from([Arc::from("a")]),
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
                Arc::from("A"),
                Arc::from([]),
                PathBuf::from("/A.txt")
            )
            .is_err()
        )
    }

    /// Test making a run with 5 segments, ABCDE where ABC and DE are sub groups.
    #[test]
    fn make_run() {
        let a =
            SegmentInfo::new(Arc::from("A"), PathBuf::from("/A.txt")).unwrap();
        let ap = RunPart::SingleSegment(&a);
        let b =
            SegmentInfo::new(Arc::from("B"), PathBuf::from("/B.txt")).unwrap();
        let bp = RunPart::SingleSegment(&b);
        let c =
            SegmentInfo::new(Arc::from("C"), PathBuf::from("/C.txt")).unwrap();
        let cp = RunPart::SingleSegment(&c);
        let d =
            SegmentInfo::new(Arc::from("D"), PathBuf::from("/D.txt")).unwrap();
        let dp = RunPart::SingleSegment(&d);
        let e =
            SegmentInfo::new(Arc::from("E"), PathBuf::from("/E.txt")).unwrap();
        let ep = RunPart::SingleSegment(&e);
        let abc_file_name = PathBuf::from("ABC.txt");
        let abcp = RunPart::Group {
            components: Arc::from([&ap, &bp, &cp]),
            display_name: Arc::from("ABC"),
            file_name: &abc_file_name,
        };
        let de_file_name = PathBuf::from("DE.txt");
        let dep = RunPart::Group {
            components: Arc::from([&dp, &ep]),
            display_name: Arc::from("DE"),
            file_name: &de_file_name,
        };
        let abcde_file_name = PathBuf::from("ABCDE.txt");
        let run = RunPart::Group {
            components: Arc::from([&abcp, &dep]),
            display_name: Arc::from("ABCDE"),
            file_name: &abcde_file_name,
        };
        let mut iter = run.run_segments().into_iter();
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
        let a =
            SegmentInfo::new(Arc::from("A"), PathBuf::from("/A.txt")).unwrap();
        let ap = RunPart::SingleSegment(&a);
        let b =
            SegmentInfo::new(Arc::from("B"), PathBuf::from("/B.txt")).unwrap();
        let bp = RunPart::SingleSegment(&b);
        let c =
            SegmentInfo::new(Arc::from("C"), PathBuf::from("/C.txt")).unwrap();
        let cp = RunPart::SingleSegment(&c);
        let d =
            SegmentInfo::new(Arc::from("D"), PathBuf::from("/D.txt")).unwrap();
        let dp = RunPart::SingleSegment(&d);
        let abf = PathBuf::from("/AB.txt");
        let abp = RunPart::Group {
            components: Arc::from([&ap, &bp]),
            display_name: Arc::from("AB"),
            file_name: &abf,
        };
        let abcf = PathBuf::from("/ABC.txt");
        let abcp = RunPart::Group {
            components: Arc::from([&abp, &cp]),
            display_name: Arc::from("ABC"),
            file_name: &abcf,
        };
        let abcdf = PathBuf::from("/ABCD.txt");
        let run_part = RunPart::Group {
            components: Arc::from([&abcp, &dp]),
            display_name: Arc::from("ABCD"),
            file_name: &abcdf,
        };
        let mut segment_specs = run_part.nested_segment_specs().into_iter();
        let mut a_spec = segment_specs.next().unwrap().into_iter();
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                a_part,
                RunPart::Group { display_name, .. },
                display_name
            ) as &str,
            "ABC"
        );
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                a_part,
                RunPart::Group { display_name, .. },
                display_name
            ) as &str,
            "AB"
        );
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                a_part,
                RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            ) as &str,
            "A"
        );
        assert!(a_spec.next().is_none());
        let mut b_spec = segment_specs.next().unwrap().into_iter();
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                b_part,
                RunPart::Group { display_name, .. },
                display_name
            ) as &str,
            "ABC"
        );
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                b_part,
                RunPart::Group { display_name, .. },
                display_name
            ) as &str,
            "AB"
        );
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 1);
        assert_eq!(
            &coerce_pattern!(
                b_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            ) as &str,
            "B"
        );
        assert!(b_spec.next().is_none());
        let mut c_spec = segment_specs.next().unwrap().into_iter();
        let (index, c_part) = c_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                c_part,
                RunPart::Group { display_name, .. },
                display_name
            ) as &str,
            "ABC"
        );
        let (index, c_part) = c_spec.next().unwrap();
        assert_eq!(index, 1);
        assert_eq!(
            &coerce_pattern!(
                c_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            ) as &str,
            "C"
        );
        assert!(c_spec.next().is_none());
        let mut d_spec = segment_specs.next().unwrap().into_iter();
        let (index, d_part) = d_spec.next().unwrap();
        assert_eq!(index, 1);
        assert_eq!(
            &coerce_pattern!(
                d_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            ) as &str,
            "D"
        );
        assert!(d_spec.next().is_none());
        let mut segments = run_part.run_segments().into_iter();
        let actual_a_segment =
            coerce_pattern!(a_part, &RunPart::SingleSegment(segment), segment);
        let expected_a_segment = segments.next().unwrap();
        assert!(ptr::eq(actual_a_segment, expected_a_segment));
        let actual_b_segment =
            coerce_pattern!(b_part, &RunPart::SingleSegment(segment), segment);
        let expected_b_segment = segments.next().unwrap();
        assert!(ptr::eq(actual_b_segment, expected_b_segment));
        let actual_c_segment =
            coerce_pattern!(c_part, &RunPart::SingleSegment(segment), segment);
        let expected_c_segment = segments.next().unwrap();
        assert!(ptr::eq(actual_c_segment, expected_c_segment));
        let actual_d_segment =
            coerce_pattern!(d_part, &RunPart::SingleSegment(segment), segment);
        let expected_d_segment = segments.next().unwrap();
        assert!(ptr::eq(actual_d_segment, expected_d_segment));
        assert!(segments.next().is_none());
    }

    /// Tests the tree method with ((AA)(AA)). This nested structure where the same
    /// part names show up back-to-back at multiple levels of nesting shows that
    /// the correct things are printed vs ignored (i.e. if the same instance of a
    /// group continues over multiple lines, it shouldn't appear more than once)
    #[test]
    fn test_tree_nested_with_repeats() {
        let a =
            SegmentInfo::new(Arc::from("A"), PathBuf::from("/a.txt")).unwrap();
        let a_part = RunPart::SingleSegment(&a);
        let aa_file_name = PathBuf::from("/aa.txt");
        let aa_part = RunPart::Group {
            components: Arc::from([&a_part, &a_part]),
            display_name: Arc::from("AA"),
            file_name: &aa_file_name,
        };
        let aaaa_file_name = PathBuf::from("/aaaa.txt");
        let aaaa_part = RunPart::Group {
            components: Arc::from([&aa_part, &aa_part]),
            display_name: Arc::from("AAAA"),
            file_name: &aaaa_file_name,
        };
        let tree_string = aaaa_part.tree();
        assert_eq!(
            tree_string.as_str(),
            "\
AAAA\tAA\tA
    \t  \tA
    \tAA\tA
    \t  \tA"
        );
    }

    /// Tests the tree function on a single segment part, which
    /// should simply print the display name of the segment.
    #[test]
    fn test_tree_single_segment() {
        let a =
            SegmentInfo::new(Arc::from("A"), PathBuf::from("/a.txt")).unwrap();
        let a_part = RunPart::SingleSegment(&a);
        assert_eq!(a_part.tree().as_str(), "A")
    }

    /// Ensures that RunPart::run works as expected when no cancelation is done. In particular,
    /// the run doesn't short-circuit and all produced SegmentRun objects are not canceled.
    #[test]
    fn test_run_ab_no_cancel() {
        let a = SegmentInfo {
            display_name: Arc::from("A"),
            file_name: PathBuf::from("/a.txt"),
        };
        let ap = RunPart::SingleSegment(&a);
        let b = SegmentInfo {
            display_name: Arc::from("B"),
            file_name: PathBuf::from("/b.txt"),
        };
        let bp = RunPart::SingleSegment(&b);
        let path = PathBuf::from("/ab.txt");
        let abp = RunPart::Group {
            components: Arc::from([&ap, &bp]),
            display_name: Arc::from("AB"),
            file_name: &path,
        };
        let (segment_run_event_sender, segment_run_event_receiver) =
            mpsc::channel();
        let (
            supplemented_segment_run_sender,
            supplemented_segment_run_receiver,
        ) = mpsc::channel();
        let input_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(5));
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
        });
        let output_thread = thread::spawn(move || {
            let mut supplemented_segment_runs = Vec::new();
            while let Ok(run) = supplemented_segment_run_receiver.recv() {
                supplemented_segment_runs.push(run);
            }
            supplemented_segment_runs
        });
        abp.run(
            segment_run_event_receiver,
            supplemented_segment_run_sender,
            false,
        )
        .unwrap();
        input_thread.join().unwrap();
        let supplemented_segment_runs = output_thread.join().unwrap();
        assert_eq!(supplemented_segment_runs.len(), 3);
        let (first_run, second_run, third_run) = (
            &supplemented_segment_runs[0],
            &supplemented_segment_runs[1],
            &supplemented_segment_runs[2],
        );
        assert_eq!(first_run.nesting, 1);
        assert_eq!(&first_run.name as &str, "A");
        assert!(!first_run.segment_run.canceled);
        assert_eq!(first_run.segment_run.deaths, 1);
        assert!(first_run.segment_run.duration().as_millis() >= 4);
        assert_eq!(second_run.nesting, 1);
        assert_eq!(&second_run.name as &str, "B");
        assert!(!second_run.segment_run.canceled);
        assert_eq!(second_run.segment_run.deaths, 0);
        assert!(second_run.segment_run.duration().as_millis() >= 4);
        assert_eq!(third_run.nesting, 0);
        assert_eq!(&third_run.name as &str, "AB");
        assert!(!third_run.segment_run.canceled);
        assert_eq!(third_run.segment_run.deaths, 1);
        assert!(third_run.segment_run.duration().as_millis() >= 9);
    }

    /// Ensures that the RunPart::run method works correctly when canceled. In
    /// particular, this test runs ((AB)C) when it is canceled during B. This
    /// should lead to:
    /// 1. A, nesting 2, not canceled
    /// 2. B, nesting 2, canceled
    /// 3. AB, nesting 1, canceled
    /// 4. ABC, nesting 0, canceled
    #[test]
    fn test_run_abc_canceled() {
        let a = SegmentInfo {
            display_name: Arc::from("A"),
            file_name: PathBuf::from("/a.txt"),
        };
        let ap = RunPart::SingleSegment(&a);
        let b = SegmentInfo {
            display_name: Arc::from("B"),
            file_name: PathBuf::from("/b.txt"),
        };
        let bp = RunPart::SingleSegment(&b);
        let ab_path = PathBuf::from("/ab.txt");
        let abp = RunPart::Group {
            components: Arc::from([&ap, &bp]),
            display_name: Arc::from("AB"),
            file_name: &ab_path,
        };
        let c = SegmentInfo {
            display_name: Arc::from("C"),
            file_name: PathBuf::from("/c.txt"),
        };
        let cp = RunPart::SingleSegment(&c);
        let abc_path = PathBuf::from("/abc.txt");
        let abcp = RunPart::Group {
            components: Arc::from([&abp, &cp]),
            display_name: Arc::from("ABC"),
            file_name: &abc_path,
        };
        let (segment_run_event_sender, segment_run_event_receiver) =
            mpsc::channel();
        let (
            supplemented_segment_run_sender,
            supplemented_segment_run_receiver,
        ) = mpsc::channel();
        let input_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            segment_run_event_sender
                .send(SegmentRunEvent::Death)
                .unwrap();
            segment_run_event_sender.send(SegmentRunEvent::End).unwrap();
            thread::sleep(Duration::from_millis(5));
            segment_run_event_sender
                .send(SegmentRunEvent::Cancel)
                .unwrap();
        });
        let output_thread = thread::spawn(move || {
            let mut supplemented_segment_runs = Vec::new();
            while let Ok(run) = supplemented_segment_run_receiver.recv() {
                supplemented_segment_runs.push(run);
            }
            supplemented_segment_runs
        });
        abcp.run(
            segment_run_event_receiver,
            supplemented_segment_run_sender,
            false,
        )
        .unwrap();
        input_thread.join().unwrap();
        let supplemented_segment_runs = output_thread.join().unwrap();
        assert_eq!(supplemented_segment_runs.len(), 4);
        let (first_run, second_run, third_run, fourth_run) = (
            &supplemented_segment_runs[0],
            &supplemented_segment_runs[1],
            &supplemented_segment_runs[2],
            &supplemented_segment_runs[3],
        );
        assert_eq!(first_run.nesting, 2);
        assert_eq!(&first_run.name as &str, "A");
        assert!(!first_run.segment_run.canceled);
        assert_eq!(first_run.segment_run.deaths, 1);
        assert!(first_run.segment_run.duration().as_millis() >= 4);
        assert_eq!(second_run.nesting, 2);
        assert_eq!(&second_run.name as &str, "B");
        assert!(second_run.segment_run.canceled);
        assert_eq!(second_run.segment_run.deaths, 0);
        assert!(second_run.segment_run.duration().as_millis() >= 4);
        assert_eq!(third_run.nesting, 1);
        assert_eq!(&third_run.name as &str, "AB");
        assert!(third_run.segment_run.canceled);
        assert_eq!(third_run.segment_run.deaths, 1);
        assert!(third_run.segment_run.duration().as_millis() >= 9);
        assert_eq!(fourth_run.nesting, 0);
        assert_eq!(&fourth_run.name as &str, "ABC");
        assert!(fourth_run.segment_run.canceled);
        assert_eq!(fourth_run.segment_run.deaths, 1);
        assert!(fourth_run.segment_run.duration().as_millis() >= 9);
    }
}
