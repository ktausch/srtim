//! Module with struct to handle CLI input, both positional arguments and keyword options
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{Error, Result};

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
    use crate::{assert_pattern, coerce_pattern};

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
        let input = Input::collect(
            ["--option2", "--option2", "a", "b", "--option1"]
                .into_iter()
                .map(String::from),
        );
        let option = coerce_pattern!(
            input,
            Err(Error::OptionProvidedTwice { option }),
            option
        );
        assert_eq!(option.as_str(), "--option2");
    }

    /// Tests that the Input::collect function returns the correct error when the
    /// same option is given twice (and the second instance occurs at the end)
    #[test]
    fn test_load_input_option_given_twice_end() {
        let input = Input::collect(
            ["--option1", "--option2", "a", "b", "--option2"]
                .into_iter()
                .map(String::from),
        );
        let option = coerce_pattern!(
            input,
            Err(Error::OptionProvidedTwice { option }),
            option
        );
        assert_eq!(option.as_str(), "--option2");
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
        let (option, values) = coerce_pattern!(
            input.extract_option_no_values("--option"),
            Err(Error::OptionExpectedNoValues { option, values }),
            (option, values)
        );
        assert_eq!(option.as_str(), "--option");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].as_str(), "value1");
        assert_eq!(values[1].as_str(), "value2");
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
        let (option, values) = coerce_pattern!(
            input.extract_root(None),
            Err(Error::OptionExpectedOneValue { option, values }),
            (option, values)
        );
        assert_eq!(option.as_str(), "--root");
        assert!(values.is_empty());
    }

    /// Tests that the Input::extract_root function returns the TooManyRootsProvided
    /// error when --root is passed with more than one associated value.
    #[test]
    fn test_extract_root_too_many_values_given() {
        let mut input = Input::collect(
            ["--root", "first", "second"].into_iter().map(String::from),
        )
        .unwrap();
        let (option, mut values) = coerce_pattern!(
            input.extract_root(None),
            Err(Error::OptionExpectedOneValue { option, values }),
            (option, values)
        );
        assert_eq!(option.as_str(), "--root");
        assert!(values.pop().unwrap().as_str() == "second");
        assert!(values.pop().unwrap().as_str() == "first");
        assert!(values.is_empty());
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
        assert_pattern!(input.extract_root(None), Err(Error::NoRootFound));
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
}
