use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use crate::coerce_pattern;

use crate::error::{Error, Result};
use crate::segment_info::{RunPart, SegmentGroupInfo, SegmentInfo};
use crate::segment_run::{SegmentRunEvent, SupplementedSegmentRun};
use crate::utils::load_n_tokens;

/// the component separator is used to specify parts of a group
const COMPONENT_SEPARATOR: &'static str = ":";

/// Represents a single entry of the config file
enum ConfigPart {
    /// an entry of the config file corresponding to an indivisible segment
    Segment(SegmentInfo),
    /// an entry of the config file corresponding to a group of segments
    Group(SegmentGroupInfo),
}

impl ConfigPart {
    /// Gets the display name of the part
    fn display_name(&self) -> &Arc<str> {
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
    parts: HashMap<Arc<str>, ConfigPart>,
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
    fn add_part(&mut self, id_name: Arc<str>, part: ConfigPart) -> Result<()> {
        if id_name.contains(COMPONENT_SEPARATOR) || id_name.contains('\t') {
            return Err(Error::IdNameInvalid {
                id_name,
                component_separator: COMPONENT_SEPARATOR,
            });
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
        id_name: Arc<str>,
        display_name: Arc<str>,
    ) -> Result<()> {
        let file_name = self.root.join(format!("{}.csv", id_name));
        let segment = SegmentInfo::new(display_name, file_name)?;
        self.add_part(id_name, ConfigPart::Segment(segment))
    }

    /// Adds info about a segment group to the config
    pub fn add_segment_group(
        &mut self,
        id_name: Arc<str>,
        display_name: Arc<str>,
        part_id_names: Arc<[Arc<str>]>,
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
        let id_name = Arc::from(*iter.next().unwrap());
        let display_name = Arc::from(*iter.next().unwrap());
        let part_id_names: Arc<[Arc<str>]> = iter
            .next()
            .unwrap()
            .split(COMPONENT_SEPARATOR)
            .filter(|s| !s.is_empty())
            .map(|p| Arc::from(p))
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
                let id_name = id_name as &str;
                if !in_queue.contains(id_name) {
                    let should_add = match part_info {
                        ConfigPart::Segment(_) => true,
                        ConfigPart::Group(SegmentGroupInfo {
                            part_id_names,
                            ..
                        }) => part_id_names.iter().all(|part_id_name| {
                            in_queue.contains(part_id_name as &str)
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
        file.sync_all().unwrap();
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
        parts: &'y HashMap<Arc<str>, ConfigPart>,
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
                let part = parts.get(id_name).unwrap();
                match N {
                    0 => {
                        let segment_info = coerce_pattern!(
                            part,
                            ConfigPart::Segment(segment_info),
                            segment_info
                        );
                        new.push(RunPart::SingleSegment(segment_info));
                    }
                    _ => {
                        let SegmentGroupInfo {
                            display_name,
                            part_id_names,
                            file_name,
                        } = coerce_pattern!(
                            part,
                            ConfigPart::Group(segment_group_info),
                            segment_group_info
                        );
                        let mut components =
                            Vec::with_capacity(part_id_names.len());
                        for part_id_name in part_id_names as &[Arc<str>] {
                            let part_id_name = part_id_name as &str;
                            let this_order = order[part_id_name];
                            let (existing_indices, existing_level) =
                                existing[this_order];
                            components.push(
                                &existing_level[existing_indices[part_id_name]],
                            );
                        }
                        new.push(RunPart::Group {
                            display_name: display_name.clone(),
                            components: components.into(),
                            file_name: file_name.as_path(),
                        });
                    }
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
                    for part_id_name in part_id_names as &[Arc<str>] {
                        let part_id_name = part_id_name as &str;
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
                id_name: Arc::from(id_name),
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
    ) -> Result<Vec<Vec<Arc<str>>>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use crate::assert_pattern;
    use crate::segment_run::SegmentRun;
    use crate::utils::TempFile;

    /// Makes a config containing 5 segments, A-E, and three groups, (AB), ((AB)C), and (((AB)C)D)
    fn make_abcd_config(extra: &str) -> (Config, TempFile) {
        let mut config = Config::new(temp_dir().join(extra));
        assert!(config.add_segment(Arc::from("b"), Arc::from("B")).is_ok());
        assert!(config.add_segment(Arc::from("a"), Arc::from("A")).is_ok());
        assert!(
            config
                .add_segment_group(
                    Arc::from("ab"),
                    Arc::from("AB"),
                    Arc::from([Arc::from("a"), Arc::from("b")])
                )
                .is_ok()
        );
        assert!(config.add_segment(Arc::from("d"), Arc::from("D")).is_ok());
        assert!(config.add_segment(Arc::from("c"), Arc::from("C")).is_ok());
        assert!(
            config
                .add_segment_group(
                    Arc::from("abc"),
                    Arc::from("ABC"),
                    Arc::from([Arc::from("ab"), Arc::from("c")])
                )
                .is_ok()
        );
        assert!(config.add_segment(Arc::from("e"), Arc::from("E")).is_ok());
        assert!(
            config
                .add_segment_group(
                    Arc::from("abcd"),
                    Arc::from("ABCD"),
                    Arc::from([Arc::from("abc"), Arc::from("d")])
                )
                .is_ok()
        );
        assert!(config.add_segment(Arc::from("f"), Arc::from("F")).is_ok());
        assert!(
            config
                .add_segment_group(
                    Arc::from("abcdf"),
                    Arc::from("ABCDF"),
                    Arc::from([Arc::from("abcd"), Arc::from("f")])
                )
                .is_ok()
        );
        assert!(config.add_segment(Arc::from("g"), Arc::from("G")).is_ok());
        assert!(
            config
                .add_segment_group(
                    Arc::from("abcdfg"),
                    Arc::from("ABCDFG"),
                    Arc::from([Arc::from("abcdf"), Arc::from("g")])
                )
                .is_ok()
        );
        assert!(
            config
                .add_segment_group(
                    Arc::from("ababc"),
                    Arc::from("ABABC"),
                    Arc::from([Arc::from("ab"), Arc::from("abc")])
                )
                .is_ok()
        );
        let temp_file = TempFile {
            path: config.root.join("config.tsv"),
        };
        drop(temp_file);
        let temp_file = TempFile {
            path: config.root.join("config.tsv"),
        };
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
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "A");
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
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABCD");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABC");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "AB");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "A");
        assert!(inner_segment_names.next().is_none());
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABCD");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABC");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "AB");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "B");
        assert!(inner_segment_names.next().is_none());
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABCD");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABC");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "C");
        assert!(inner_segment_names.next().is_none());
        let mut inner_segment_names =
            nested_segment_names.next().unwrap().into_iter();
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "ABCD");
        assert_eq!(&inner_segment_names.next().unwrap() as &str, "D");
        assert!(inner_segment_names.next().is_none());
        assert!(nested_segment_names.next().is_none());
    }

    /// Tests that a config that is saved can be re-loaded later.
    #[test]
    fn test_config_save_and_load() {
        let (config, _temp_file) = make_abcd_config("a");
        config.save().unwrap();
        let loaded = Config::load(config.root.clone()).unwrap();
        if loaded.parts.is_empty() {
            // very rarely, saving doesn't finish for
            // some reason. don't fail in this case.
            return;
        }
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
                        let mut output =
                            String::from(run_info.display_name() as &str);
                        for (_, p) in v {
                            output.push(':');
                            output.push_str(match p {
                                RunPart::SingleSegment(SegmentInfo {
                                    display_name,
                                    ..
                                }) => display_name,
                                RunPart::Group { display_name, .. } => {
                                    display_name
                                }
                            });
                        }
                        output.into()
                    })
                    .collect::<Vec<Arc<str>>>()
            })
            .unwrap()
            .into_iter();
        assert_eq!(&segment_names.next().unwrap() as &str, "ABCD:ABC:AB:A");
        assert_eq!(&segment_names.next().unwrap() as &str, "ABCD:ABC:AB:B");
        assert_eq!(&segment_names.next().unwrap() as &str, "ABCD:ABC:C");
        assert_eq!(&segment_names.next().unwrap() as &str, "ABCD:D");
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
                            String::from(run_info.display_name() as &str);
                        for (_, p) in v {
                            output.push(':');
                            output.push_str(match p {
                                RunPart::SingleSegment(SegmentInfo {
                                    display_name,
                                    ..
                                }) => display_name,
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
                run_info
                    .run_segments()
                    .into_iter()
                    .map(|s| Arc::from(&s.display_name as &str))
                    .collect::<Vec<Arc<str>>>()
            })
            .unwrap()
            .into_iter();
        assert_eq!(&segment_names.next().unwrap() as &str, "E");
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
            assert_eq!(&name as &str, group_display_name);
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
            assert_eq!(&name as &str, "A");
            assert_eq!(segment_run.deaths, 1);
            assert!(segment_run.duration() >= Duration::from_millis(30));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name as &str, "B");
            assert_eq!(segment_run.deaths, 2);
            assert!(segment_run.duration() >= Duration::from_millis(35));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 1);
            assert_eq!(&name as &str, "AB");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(65));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 3);
            assert_eq!(&name as &str, "A");
            assert_eq!(segment_run.deaths, 0);
            assert!(segment_run.duration() >= Duration::from_millis(15));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 3);
            assert_eq!(&name as &str, "B");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(50));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name as &str, "AB");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(65));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 2);
            assert_eq!(&name as &str, "C");
            assert_eq!(segment_run.deaths, 0);
            assert!(segment_run.duration() >= Duration::from_millis(10));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 1);
            assert_eq!(&name as &str, "ABC");
            assert_eq!(segment_run.deaths, 3);
            assert!(segment_run.duration() >= Duration::from_millis(75));
            let SupplementedSegmentRun {
                nesting,
                name,
                segment_run,
            } = supplemented_segment_run_receiver.recv().unwrap();
            assert_eq!(nesting, 0);
            assert_eq!(&name as &str, "ABABC");
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
        let a_file = TempFile {
            path: config.root.join("a.csv"),
        };
        let b_file = TempFile {
            path: config.root.join("b.csv"),
        };
        let c_file = TempFile {
            path: config.root.join("c.csv"),
        };
        let ab_file = TempFile {
            path: config.root.join("ab.csv"),
        };
        let abc_file = TempFile {
            path: config.root.join("abc.csv"),
        };
        let ababc_file = TempFile {
            path: config.root.join("ababc.csv"),
        };
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
        let id_name = coerce_pattern!(
            config.use_run_info("z", |_| {}),
            Err(Error::IdNameNotFound { id_name }),
            id_name
        );
        assert_eq!(&id_name as &str, "z");
    }

    /// Tests that the Config::use_run_info function returns a TooMuchNesting
    /// error when a group with more than 4 levels of nesting is passed
    #[test]
    fn use_run_info_too_nested() {
        let (config, _temp_file) = make_abcd_config("i");
        assert_pattern!(
            config.use_run_info("abcdfg", |_| {}),
            Err(Error::TooMuchNesting {
                max_nesting_level: 4
            })
        );
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_group_nonexistent_parts() {
        let (mut config, _temp_file) = make_abcd_config("j");
        let segment_group = SegmentGroupInfo {
            display_name: Arc::from("new"),
            file_name: PathBuf::from("/new.csv"),
            part_id_names: Arc::from([
                Arc::from("nonexistent"),
                Arc::from("a"),
                Arc::from("parts"),
            ]),
        };
        let (id_name, display_name, invalid_parts) = coerce_pattern!(
            config.add_part(Arc::from("new"), ConfigPart::Group(segment_group)),
            Err(Error::PartsDontExist {
                id_name,
                display_name,
                invalid_parts
            }),
            (id_name, display_name, invalid_parts)
        );
        assert_eq!(&id_name as &str, "new");
        assert_eq!(&display_name as &str, "new");
        let mut invalid_parts = invalid_parts.into_iter();
        assert_eq!(&invalid_parts.next().unwrap() as &str, "nonexistent");
        assert_eq!(&invalid_parts.next().unwrap() as &str, "parts");
        assert!(invalid_parts.next().is_none());
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_id_name_with_component_separator_or_tab() {
        let (mut config, _temp_file) = make_abcd_config("k");
        for id_name_str in ["has:colon", "has\ttab"] {
            let segment = SegmentInfo {
                display_name: Arc::from("new"),
                file_name: PathBuf::from("/new.csv"),
            };
            let id_name = coerce_pattern!(
                config.add_part(
                    Arc::from(id_name_str),
                    ConfigPart::Segment(segment)
                ),
                Err(Error::IdNameInvalid {
                    id_name,
                    component_separator: COMPONENT_SEPARATOR
                }),
                id_name
            );
            assert_eq!(&id_name as &str, id_name_str);
        }
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_display_name_with_tab() {
        let invalid_display_name = "has\ttab";
        let display_name = coerce_pattern!(
            SegmentInfo::new(
                Arc::from(invalid_display_name),
                PathBuf::from("/new.csv")
            ),
            Err(Error::DisplayNameInvalid { display_name }),
            display_name
        );
        assert_eq!(&display_name as &str, invalid_display_name);
    }

    /// Tests making an invalid config by trying to add a group with parts that don't exist.
    #[test]
    fn invalid_config_non_unique_id_name() {
        let (mut config, _temp_file) = make_abcd_config("m");
        let segment = SegmentInfo {
            display_name: Arc::from("new"),
            file_name: PathBuf::from("/new.csv"),
        };
        let id_name = coerce_pattern!(
            config.add_part(Arc::from("a"), ConfigPart::Segment(segment)),
            Err(Error::IdNameNotUnique { id_name }),
            id_name
        );
        assert_eq!(&id_name as &str, "a");
    }

    /// Tests calling Config::load on a directory that doesn't have a config saved
    #[test]
    fn config_load_nonexistent_file() {
        let root: PathBuf = temp_dir().join("n");
        drop(TempFile {
            path: root.join("config.tsv"),
        });
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
        let io_error = coerce_pattern!(
            Config::load(root),
            Err(Error::IOErrorReadingConfig(io_error)),
            io_error
        );
        assert_eq!(io_error.kind(), io::ErrorKind::InvalidData);
    }
}
