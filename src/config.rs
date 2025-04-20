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
pub enum ConfigPart {
    /// an entry of the config file corresponding to an indivisible segment
    Segment(SegmentInfo),
    /// an entry of the config file corresponding to a group of segments
    Group(SegmentGroupInfo),
}

impl ConfigPart {
    /// Gets the display name of the part
    #[allow(dead_code)]
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

    /// Read-only view of the actual segments and groups in the config
    pub fn parts(&self) -> &HashMap<Arc<str>, ConfigPart> {
        &self.parts
    }

    /// finds the config file inside the given root directory
    pub fn file_name(root: &Path) -> PathBuf {
        root.join(PathBuf::from(Self::FILE_NAME))
    }

    /// Transforms a part to lean it as much as possible in terms
    /// of the number of allocations it implies. If the part is a
    /// single segment, it is returned as is. If the part is a
    /// segment group, its part name `Arc<str>` objects are replaced
    /// by clones of the IDs they correspond to so that the actual
    /// string allocations only happen once.
    fn thin_out_part(
        &self,
        id_name: Arc<str>,
        part: ConfigPart,
    ) -> Result<(Arc<str>, ConfigPart)> {
        match part {
            ConfigPart::Segment(_) => Ok((id_name, part)),
            ConfigPart::Group(SegmentGroupInfo {
                display_name,
                part_id_names,
                file_name,
            }) => {
                if part_id_names.iter().any(|s| !self.parts.contains_key(s)) {
                    return Err(Error::PartsDontExist {
                        id_name,
                        display_name,
                        invalid_parts: part_id_names
                            .iter()
                            .filter(|s| !self.parts.contains_key(*s))
                            .map(|s| s.clone())
                            .collect(),
                    });
                }
                let new_part_id_names: Arc<[Arc<str>]> = (&part_id_names
                    as &[Arc<str>])
                    .into_iter()
                    .map(|part_id_name| {
                        self.parts
                            .get_key_value(part_id_name)
                            .unwrap()
                            .0
                            .clone()
                    })
                    .collect();
                Ok((
                    id_name,
                    ConfigPart::Group(SegmentGroupInfo {
                        display_name,
                        part_id_names: new_part_id_names,
                        file_name,
                    }),
                ))
            }
        }
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
        let (id_name, part) = self.thin_out_part(id_name, part)?;
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

    /// For each node, find ID names of all segment groups that contain
    /// it directly. Transitive descent is not considered here.
    fn first_order_dependencies(&self) -> HashMap<Arc<str>, Arc<[Arc<str>]>> {
        let mut pre_processed: HashMap<Arc<str>, Vec<Arc<str>>> =
            HashMap::new();
        for (id_name, part) in &self.parts {
            if let ConfigPart::Group(segment_group_info) = part {
                for part_id_name in
                    &segment_group_info.part_id_names as &[Arc<str>]
                {
                    match pre_processed.entry(part_id_name.clone()) {
                        Entry::Occupied(mut entry) => {
                            entry.get_mut().push(id_name.clone());
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(vec![id_name.clone()]);
                        }
                    }
                }
            }
        }
        let mut result = HashMap::new();
        for (id_name, dependencies) in pre_processed {
            result.insert(id_name, dependencies.into());
        }
        result
    }

    /// Gets the ID names of all nodes that have the given node as a descendant.
    /// Fails if the ID name is not found in the config at all.
    fn node_dependencies(&self, id_name: &str) -> Result<Arc<[Arc<str>]>> {
        let first_order_dependencies = self.first_order_dependencies();
        let mut result = HashSet::new();
        let mut processed = HashSet::new();
        let mut to_process: Vec<Arc<str>> = Vec::new();
        to_process.push(Arc::from(id_name));
        while !to_process.is_empty() {
            to_process = {
                let mut new_to_process: Vec<Arc<str>> = Vec::new();
                for element in to_process {
                    let element_clone = element.clone();
                    if !processed.insert(element) {
                        continue;
                    }
                    let dependencies =
                        match first_order_dependencies.get(&element_clone) {
                            None => continue,
                            Some(dependencies) => dependencies as &[Arc<str>],
                        };
                    for dependency in dependencies {
                        new_to_process.push(dependency.clone());
                        result.insert(dependency.clone());
                    }
                }
                new_to_process
            }
        }
        match self.parts.contains_key(id_name) {
            true => Ok(result.into_iter().collect()),
            false => Err(Error::IdNameNotFound {
                id_name: Arc::from(id_name),
            }),
        }
    }

    /// Deletes a segment or group of segments. Will fail if the part
    /// is an element in the run tree of a different segment group or
    /// if no segment or segment group has the given id name
    pub fn delete_part(
        &mut self,
        id_name: &str,
    ) -> Result<(Arc<str>, ConfigPart)> {
        let dependencies = self.node_dependencies(id_name)?;
        match dependencies.is_empty() {
            true => {
                // unwrap is safe here because if id_name wasn't in self.parts,
                // then dependency calculation would return an error and would
                // short circuit above this match expression
                Ok(self.parts.remove_entry(id_name).unwrap())
            }
            false => Err(Error::CannotDeletePart {
                part_id_name: Arc::from(id_name),
                constraining_parts: dependencies,
            }),
        }
    }

    /// Deletes the part with the given ID name.
    /// Also deletes all parts that depend on it.
    pub fn delete_part_recursive(
        &mut self,
        id_name: &str,
    ) -> Result<HashMap<Arc<str>, ConfigPart>> {
        let mut result = HashMap::new();
        let transitive_dependencies = self.node_dependencies(id_name)?;
        for transitive_dependency in &transitive_dependencies as &[Arc<str>] {
            let (transitive_dependency, deleted_part) =
                self.parts.remove_entry(transitive_dependency).unwrap();
            result.insert(transitive_dependency, deleted_part);
        }
        let (id_name, final_part) = self.parts.remove_entry(id_name).unwrap();
        result.insert(id_name, final_part);
        Ok(result)
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

    /// Prints a string with one line per segment. Each segment is specified by its full
    /// branch, i.e. <group display name>, <group display name>, <segment display name>
    /// separated by tabs. However, if a group continues from one segment to the next, it
    /// is omitted and replaced by a number of spaces equal to the length of the name.
    pub fn run_tree(&'a self, id_name: &'a str) -> Result<String> {
        self.use_run_info(id_name, |root| root.tree())
    }

    /// Lists all segments and groups.
    pub fn list(&'a self) -> String {
        let mut sorted_keys = Vec::with_capacity(self.parts.len());
        for key in self.parts.keys() {
            sorted_keys.push(key as &str);
        }
        sorted_keys.sort();
        let mut result = String::new();
        let mut sorted_keys = sorted_keys.into_iter();
        let mut key = match sorted_keys.next() {
            None => {
                return result;
            }
            Some(key) => key,
        };
        loop {
            let (display_name, is_group) = match &self.parts[key] {
                ConfigPart::Segment(segment_info) => {
                    (&segment_info.display_name, false)
                }
                ConfigPart::Group(segment_group_info) => {
                    (&segment_group_info.display_name, true)
                }
            };
            result.push_str(key);
            result.push_str(": ");
            result.push_str(display_name);
            result.push('\t');
            result.push_str(if is_group { "Group" } else { "Segment" });
            key = match sorted_keys.next() {
                None => break,
                Some(key) => key,
            };
            result.push('\n');
        }
        result
    }

    /// Receives SegmentRunEvents and generates SupplementedSegmentRuns
    /// for all segments and groups when they end.
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

    /// Makes a config containing
    /// - A
    /// - B
    /// - C
    /// - D
    /// - E
    /// - F
    /// - G
    /// - (AB)
    /// - ((AB)C)
    /// - (((AB)C)D)
    /// - ((((AB)C)D)F)
    /// - (((((AB)C)D)F)G)
    /// - ((AB)((AB)C))
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
                let mut nested_segment_specs =
                    run_info.nested_segment_specs().into_iter();
                assert!(nested_segment_specs.next().unwrap().is_empty());
                assert!(nested_segment_specs.next().is_none());
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

    /// Tests that the Config::first_order_dependencies method correctly finds
    /// all direct containment relationships (i.e. the direct parts of all
    /// segment groups in the config)
    #[test]
    fn test_first_order_dependencies() {
        let (config, _temp_file) =
            make_abcd_config("test_first_order_dependencies");
        let all_dependencies = config.first_order_dependencies();
        let get_dependencies = |id_name: &str| -> HashSet<&str> {
            let dependencies =
                all_dependencies.get(id_name).unwrap() as &[Arc<str>];
            let length = dependencies.len();
            let as_set: HashSet<&str> =
                dependencies.into_iter().map(|name| name as &str).collect();
            assert_eq!(as_set.len(), length);
            as_set
        };
        let dependencies = get_dependencies("a");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("ab"));
        let dependencies = get_dependencies("b");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("ab"));
        let dependencies = get_dependencies("c");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("abc"));
        let dependencies = get_dependencies("d");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("abcd"));
        assert!(all_dependencies.get("e").is_none());
        let dependencies = get_dependencies("f");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("abcdf"));
        let dependencies = get_dependencies("g");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("abcdfg"));
        let dependencies = get_dependencies("ab");
        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.contains("abc"));
        assert!(dependencies.contains("ababc"));
        let dependencies = get_dependencies("abc");
        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.contains("abcd"));
        assert!(dependencies.contains("ababc"));
        let dependencies = get_dependencies("abcd");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("abcdf"));
        let dependencies = get_dependencies("abcdf");
        assert_eq!(dependencies.len(), 1);
        assert!(dependencies.contains("abcdfg"));
        assert!(all_dependencies.get("abcdfg").is_none());
        assert!(all_dependencies.get("ababc").is_none());
    }

    /// Tests that the Config::node_dependencies method returns an IdNameNotFound
    /// error if an ID name that doesn't exist is passed to it.
    #[test]
    fn test_node_dependencies_non_existent_name() {
        let (config, _temp_file) =
            make_abcd_config("test_node_dependencies_non_existent_name");
        assert_eq!(
            &coerce_pattern!(
                config.node_dependencies("doesntexist"),
                Err(Error::IdNameNotFound { id_name }),
                id_name
            ) as &str,
            "doesntexist"
        );
    }

    /// Tests that the Config::node_dependencies method returns an empty
    /// Arc<[Arc<str>]> if an ID name that can be deleted without conflict
    /// is past.
    #[test]
    fn test_node_dependencies_no_parents() {
        let (config, _temp_file) =
            make_abcd_config("test_node_dependencies_no_parents");
        assert!(config.node_dependencies("ababc").unwrap().is_empty());
    }

    /// Tests that the Config::node_dependencies method returns a list
    /// of all groups that a segment is a part of when those groups
    /// are not themselves part of groups.
    #[test]
    fn test_node_dependencies_direct_parent() {
        let (config, _temp_file) =
            make_abcd_config("test_node_dependencies_direct_parent");
        let dependencies = config.node_dependencies("abcdf").unwrap();
        assert_eq!(dependencies.len(), 1);
        assert_eq!(&dependencies[0] as &str, "abcdfg");
    }

    /// Tests that, in the general case of passing in any ID name to
    /// Config::node_dependencies, it returns a list of all segment groups
    /// that contain the part, even in a nested way (e.g. being part of a group
    /// that is part of another group)
    #[test]
    fn test_node_dependencies_transitive_parents() {
        let (config, _temp_file) =
            make_abcd_config("test_node_dependencies_transitive_parents");
        let all_dependencies = config.node_dependencies("ab").unwrap();
        let dependencies: HashSet<&str> = (&all_dependencies as &[Arc<str>])
            .into_iter()
            .map(|name| name as &str)
            .collect();
        assert_eq!(dependencies.len(), 5);
        assert!(dependencies.contains("abc"));
        assert!(dependencies.contains("ababc"));
        assert!(dependencies.contains("abcdf"));
        assert!(dependencies.contains("abcdfg"));
    }

    /// Tests that an IdNameNotFound error is returned if Config::delete_part
    /// is called with an ID name that doesn't correspond to any saved parts.
    #[test]
    fn test_delete_part_non_existent_name() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_non_existent_name");
        assert_eq!(
            &coerce_pattern!(
                config.delete_part("doesntexist"),
                Err(Error::IdNameNotFound { id_name }),
                id_name
            ) as &str,
            "doesntexist"
        );
    }

    /// Tests that the Config::delete_part method successfully deletes
    /// single segments that are not part of any groups.
    #[test]
    fn test_delete_part_dependencyless_segment() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_dependencyless_segment");
        let (id_name, deleted) = coerce_pattern!(
            config.delete_part("e"),
            Ok((id_name, ConfigPart::Segment(segment_info))),
            (id_name, segment_info)
        );
        assert_eq!(&id_name as &str, "e");
        assert_eq!(&deleted.display_name as &str, "E");
    }

    /// Tests that the Config::delete_part method successfully deletes
    /// segment groups that are not part of any other groups.
    #[test]
    fn test_delete_part_dependencyless_segment_group() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_dependencyless_segment_group");
        let (id_name, deleted) = coerce_pattern!(
            config.delete_part("ababc"),
            Ok((id_name, ConfigPart::Group(segment_group_info))),
            (id_name, segment_group_info)
        );
        assert_eq!(&id_name as &str, "ababc");
        assert_eq!(&deleted.display_name as &str, "ABABC");
        let part_id_names: Vec<&str> = deleted
            .part_id_names
            .into_iter()
            .map(|part_id_name| part_id_name as &str)
            .collect();
        assert_eq!(part_id_names.len(), 2);
        assert_eq!(part_id_names[0], "ab");
        assert_eq!(part_id_names[1], "abc");
    }

    /// Tests that the Config::delete_part method returns an error
    /// with all groups a part is a part of if it can't be deleted.
    #[test]
    fn test_delete_part_only_direct_parent() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_only_direct_parent");
        let (part_id_name, constraining_parts) = coerce_pattern!(
            config.delete_part("abcdf"),
            Err(Error::CannotDeletePart {
                part_id_name,
                constraining_parts
            }),
            (part_id_name, constraining_parts)
        );
        assert_eq!(&part_id_name as &str, "abcdf");
        assert_eq!(constraining_parts.len(), 1);
        assert_eq!(&constraining_parts[0] as &str, "abcdfg");
    }

    /// Tests that the Config::delete_part method returns an error
    /// with all groups a part is a part of if it can't be deleted.
    /// Same as test_delete_part_only_direct_parent except this one
    /// also tests that transitive membership of groups is reflected
    /// in returned error.
    #[test]
    fn test_delete_part_transitive_parents() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_transitive_parents");
        let (part_id_name, constraining_parts) = coerce_pattern!(
            config.delete_part("abc"),
            Err(Error::CannotDeletePart {
                part_id_name,
                constraining_parts
            }),
            (part_id_name, constraining_parts)
        );
        assert_eq!(&part_id_name as &str, "abc");
        assert_eq!(constraining_parts.len(), 4);
        let constraining_parts: HashSet<&str> = (&constraining_parts
            as &[Arc<str>])
            .into_iter()
            .map(|constraining_part| constraining_part as &str)
            .collect();
        assert_eq!(constraining_parts.len(), 4);
        assert!(constraining_parts.contains("ababc"));
        assert!(constraining_parts.contains("abcd"));
        assert!(constraining_parts.contains("abcdf"));
        assert!(constraining_parts.contains("abcdfg"));
    }

    /// Tests that the Config::delete_part_recursive method returns
    /// an IdNameNotFound error if passed a nonexistent ID name
    #[test]
    fn test_delete_part_recursive_nonexistent_name() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_recursive_nonexistent_name");
        assert_eq!(
            &coerce_pattern!(
                config.delete_part_recursive("doesnt_exist"),
                Err(Error::IdNameNotFound { id_name }),
                id_name
            ) as &str,
            "doesnt_exist"
        );
    }

    /// Tests the Config::delete_part_recursive to ensure that it properly deletes
    /// all parts that depend on the given part by using ((AB)C) in the abcd config
    #[test]
    fn test_delete_part_recursive_abc() {
        let (mut config, _temp_file) =
            make_abcd_config("test_delete_part_recursive_complex");
        assert_eq!(config.parts.len(), 13);
        let deleted = config.delete_part_recursive("abc").unwrap();
        assert_eq!(deleted.len(), 5);
        assert_eq!(&deleted.get("abc").unwrap().display_name() as &str, "ABC");
        assert_eq!(
            &deleted.get("abcd").unwrap().display_name() as &str,
            "ABCD"
        );
        assert_eq!(
            &deleted.get("abcdf").unwrap().display_name() as &str,
            "ABCDF"
        );
        assert_eq!(
            &deleted.get("abcdfg").unwrap().display_name() as &str,
            "ABCDFG"
        );
        assert_eq!(
            &deleted.get("ababc").unwrap().display_name() as &str,
            "ABABC"
        );
        assert_eq!(config.parts.len(), 8);
        assert!(config.parts.contains_key("a"));
        assert!(config.parts.contains_key("b"));
        assert!(config.parts.contains_key("ab"));
        assert!(config.parts.contains_key("c"));
        assert!(config.parts.contains_key("d"));
        assert!(config.parts.contains_key("e"));
        assert!(config.parts.contains_key("f"));
        assert!(config.parts.contains_key("g"));
    }

    /// Ensures that the Config::delete_part_recursive
    #[test]
    fn test_delete_part_recursive_aabcaa() {
        let mut config = Config::new(PathBuf::from("/my_root"));
        config.add_segment(Arc::from("a"), Arc::from("A")).unwrap();
        config.add_segment(Arc::from("b"), Arc::from("B")).unwrap();
        config.add_segment(Arc::from("c"), Arc::from("C")).unwrap();
        config
            .add_segment_group(
                Arc::from("bc"),
                Arc::from("BC"),
                Arc::from([Arc::from("b"), Arc::from("c")]),
            )
            .unwrap();
        config
            .add_segment_group(
                Arc::from("aa"),
                Arc::from("AA"),
                Arc::from([Arc::from("a"), Arc::from("a")]),
            )
            .unwrap();
        config
            .add_segment_group(
                Arc::from("abc"),
                Arc::from("ABC"),
                Arc::from([Arc::from("a"), Arc::from("bc")]),
            )
            .unwrap();
        config
            .add_segment_group(
                Arc::from("aabcaa"),
                Arc::from("AABCAA"),
                Arc::from([Arc::from("a"), Arc::from("abc"), Arc::from("aa")]),
            )
            .unwrap();
        let deleted = config.delete_part_recursive("a").unwrap();
        assert_eq!(deleted.len(), 4);
        assert_eq!(&deleted.get("a").unwrap().display_name() as &str, "A");
        assert_eq!(&deleted.get("aa").unwrap().display_name() as &str, "AA");
        assert_eq!(&deleted.get("abc").unwrap().display_name() as &str, "ABC");
        assert_eq!(
            &deleted.get("aabcaa").unwrap().display_name() as &str,
            "AABCAA"
        );
        assert_eq!(config.parts.len(), 3);
        assert!(config.parts.contains_key("b"));
        assert!(config.parts.contains_key("c"));
        assert!(config.parts.contains_key("bc"));
    }

    /// Tests that the list string of an empty config is the empty string.
    #[test]
    fn test_list_empty() {
        let config = Config::new(temp_dir().join("test_list_empty"));
        assert!(config.list().is_empty());
    }

    /// Tests that the list string of a config orders parts alphabetically
    /// with ID name and includes the display name and whether it is a
    /// segment or a group of segments.
    #[test]
    fn test_list() {
        let (config, _temp_file) = make_abcd_config("test_list");
        assert_eq!(
            config.list().as_str(),
            "\
a: A\tSegment
ab: AB\tGroup
ababc: ABABC\tGroup
abc: ABC\tGroup
abcd: ABCD\tGroup
abcdf: ABCDF\tGroup
abcdfg: ABCDFG\tGroup
b: B\tSegment
c: C\tSegment
d: D\tSegment
e: E\tSegment
f: F\tSegment
g: G\tSegment"
        );
    }
}
