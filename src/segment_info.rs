//! Module with
//! 1. owning-types defining segments and segment groups
//! 2. non-owning-type defining a part of a run in a tree like structure
use std::collections::{HashMap, hash_map::Entry};
use std::path::{Path, PathBuf};
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
            display_name: String::from(display_name),
        })
    } else {
        Ok(())
    }
}

/// Logistical information about a single segment, including the name that
/// should be displayed and the file in which its information is stored.
pub struct SegmentInfo {
    /// name that should be displayed for this segment
    pub display_name: String,
    /// absolute path to file containing this segment's info
    pub file_name: PathBuf,
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
pub struct SegmentGroupInfo {
    /// name that should be displayed for this group
    pub display_name: String,
    /// non-empty list of ID names of the parts that make up this group
    pub part_id_names: Vec<String>,
    /// absolute path to file containing information about this group's runs
    pub file_name: PathBuf,
}

impl SegmentGroupInfo {
    /// Creates a new SegmentGroupInfo, ensuring file_name is an absolute path and part_id_names is non-empty
    pub fn new(
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
pub enum RunPart<'a> {
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

/// Owned display and path names for a given run part
pub struct PartCore {
    /// the human-readable name of the part
    pub display_name: String,
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
    pub fn display_name(&'a self) -> &'a str {
        match self {
            &RunPart::Group { display_name, .. } => display_name,
            &RunPart::SingleSegment(SegmentInfo { display_name, .. }) => {
                display_name.as_str()
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
    pub fn nested_segment_names(&self) -> Vec<Vec<PartCore>> {
        self.nested_segment_specs()
            .into_iter()
            .map(|segment_spec| {
                segment_spec
                    .into_iter()
                    .map(|(index, part)| PartCore {
                        display_name: String::from(part.display_name()),
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
                        ..
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    use crate::{assert_pattern, coerce_pattern};

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
        assert_pattern!(
            a_part,
            &RunPart::Group {
                display_name: "ABC",
                ..
            }
        );
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_pattern!(
            a_part,
            &RunPart::Group {
                display_name: "AB",
                ..
            }
        );
        let (index, a_part) = a_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(
            coerce_pattern!(
                a_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            )
            .as_str(),
            "A"
        );
        assert!(a_spec.next().is_none());
        let mut b_spec = segment_specs.next().unwrap().into_iter();
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_pattern!(
            b_part,
            &RunPart::Group {
                display_name: "ABC",
                ..
            }
        );
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_pattern!(
            b_part,
            &RunPart::Group {
                display_name: "AB",
                ..
            }
        );
        let (index, b_part) = b_spec.next().unwrap();
        assert_eq!(index, 1);
        assert_eq!(
            coerce_pattern!(
                b_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            )
            .as_str(),
            "B"
        );
        assert!(b_spec.next().is_none());
        let mut c_spec = segment_specs.next().unwrap().into_iter();
        let (index, c_part) = c_spec.next().unwrap();
        assert_eq!(index, 0);
        assert_pattern!(
            c_part,
            &RunPart::Group {
                display_name: "ABC",
                ..
            }
        );
        let (index, c_part) = c_spec.next().unwrap();
        assert_eq!(index, 1);
        assert_eq!(
            coerce_pattern!(
                c_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            )
            .as_str(),
            "C"
        );
        assert!(c_spec.next().is_none());
        let mut d_spec = segment_specs.next().unwrap().into_iter();
        let (index, d_part) = d_spec.next().unwrap();
        assert_eq!(index, 1);
        assert_eq!(
            coerce_pattern!(
                d_part,
                &RunPart::SingleSegment(SegmentInfo { display_name, .. }),
                display_name
            )
            .as_str(),
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
}
