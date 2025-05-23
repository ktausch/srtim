//! Module containing utils used throughout the library crate.
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

/// Struct that iterates through two separate iterators to find all the elements on the front that are equal
struct ZipSame<T, U, V>
where
    T: PartialEq,
    U: Iterator<Item = T>,
    V: Iterator<Item = T>,
{
    /// two iterators to check how long they are equal
    iterators: (U, V),
    /// whether or not the ZipSame object has already found a difference
    /// between the iterators (or the end of either of them)
    depleted: bool,
}

/// ZipSame implements the same Iterator trait as U (source iterator
/// type) because it returns elements from the source iterators.
impl<T, U, V> Iterator for ZipSame<T, U, V>
where
    T: PartialEq,
    U: Iterator<Item = T>,
    V: Iterator<Item = T>,
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
            if let Some(first) = first {
                if let Some(second) = second {
                    if first == second {
                        return Some(first);
                    }
                }
            }
            self.depleted = true;
            None
        }
    }
}

/// Creates an iterator that will iterate through first and second (which iterate over
/// the same type) and yield elements that are equal until one of the iterators is exhausted
/// or the iterator's are advanced to the point that they yield different elements.
pub fn zip_same<T, U, V>(first: U, second: V) -> impl Iterator<Item = T>
where
    T: PartialEq,
    U: Iterator<Item = T>,
    V: Iterator<Item = T>,
{
    ZipSame {
        iterators: (first, second),
        depleted: false,
    }
}

/// Loads n string slices from the given iterator, returning an error
/// containing whether there were too many or too few tokens available
pub fn load_n_tokens<'a>(
    tokens: impl Iterator<Item = &'a str>,
    expected_number: usize,
) -> std::result::Result<Arc<[&'a str]>, bool> {
    let result: Arc<[&'a str]> = tokens.take(expected_number + 1).collect();
    let length = result.len();
    if length == expected_number {
        Ok(result)
    } else {
        Err(length > expected_number)
    }
}

/// Thin wrapper around a PathBuf that removes any file at the path upon drop
pub struct TempFile {
    /// the file being tracked which will be deleted upon dropping
    pub path: PathBuf,
}

impl TempFile {
    /// writes a new file with the given contents and returns
    /// a TempFile that will remove it upon being dropped
    #[allow(dead_code)]
    pub fn with_contents<'b>(
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
        match fs::remove_file(&self.path) {
            _ => {}
        }
    }
}

/// A struct that supplements an iterator with storage
/// of the most recently yielded element.
pub struct IteratorWithMemory<T, I>
where
    I: Iterator<Item = T>,
{
    /// the most recently yielded element from the iterator.
    /// None if haven't advanced yet or iterator is exhausted
    current: Option<T>,
    /// underlying iterator
    iterator: I,
}

impl<T, I> IteratorWithMemory<T, I>
where
    I: Iterator<Item = T>,
{
    pub fn current(&self) -> Option<&T> {
        self.current.as_ref()
    }

    /// Constructs a new IteratorWithMemory out of the given iterator
    pub fn new(iterator: I) -> Self {
        Self {
            current: None,
            iterator,
        }
    }

    /// Advances the iterator and updates the element in memory
    pub fn advance(&mut self) {
        self.current = self.iterator.next();
    }

    /// Advances the iterator zero or more times until either
    /// 1) the iterator is exhausted; or
    /// 2) the element in memory satisfies the given condition
    pub fn advance_until<C>(&mut self, condition: C)
    where
        C: Fn(&T) -> bool,
    {
        loop {
            if let Some(current) = &self.current {
                if condition(current) {
                    return;
                }
            }
            self.advance();
            if self.current.is_none() {
                return;
            }
        }
    }
}

/// Panics if the given expression doesn't match the given pattern.
/// Optionally, the caller can provide a panic message.
///
/// Example:
///
/// ```rust
/// use srtim::assert_pattern;
/// let y = Some(1);
/// // equivalent to if !y.is_some() {panic!("custom error message");}
/// assert_pattern!(y, Some(_), "custom error message");
/// ```
#[macro_export]
macro_rules! assert_pattern {
    ($expression:expr, $pattern:pat) => {
        assert_pattern!(
            $expression,
            $pattern,
            "expression didn't match pattern in assert_pattern!"
        );
    };
    ($expression:expr, $pattern:pat, $message:expr) => {
        match $expression {
            $pattern => {}
            _ => panic!("{}", $message),
        }
    };
}

/// Panics if the given expression doesn't match the given pattern. If
/// the pattern matches, the third argument is used to bind to elements
/// from the pattern. Optionally, the caller can provide a panic message.
///
/// Example:
///
/// ```rust
/// use srtim::coerce_pattern;
/// let y = Some(1);
/// // equivalent to let x = y.expect("custom error message");
/// let z = coerce_pattern!(y, Some(x), x, "custom error message");
/// ```
#[macro_export]
macro_rules! coerce_pattern {
    ($expression:expr, $pattern:pat, $result:expr) => {
        coerce_pattern!(
            $expression,
            $pattern,
            $result,
            "expression didn't match pattern in coerce_pattern!"
        )
    };
    ($expression:expr, $pattern:pat, $result:expr, $message:expr) => {
        match $expression {
            $pattern => $result,
            _ => panic!("{}", $message),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    /// zip_same creates an empty iterator when both source iterators are empty
    #[test]
    fn zip_same_both_empty() {
        let x = ([] as [i32; 0]).into_iter();
        let y = ([] as [i32; 0]).into_iter();
        let mut z = zip_same(x, y);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
    }

    /// zip_same creates an empty iterator when one of them is empty
    #[test]
    fn zip_same_first_empty() {
        let mut y_yielded = Vec::new();
        let x = ([] as [i32; 0]).into_iter();
        let y = [1, 2].into_iter().map(|value| {
            y_yielded.push(value);
            value
        });
        let mut z = zip_same(x, y);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
        drop(z);
        assert_eq!(y_yielded.len(), 1);
        let mut y_yielded_iter = y_yielded.into_iter();
        assert_eq!(y_yielded_iter.next().unwrap(), 1);
    }

    /// zip_same creates an empty iterator when one of them is empty
    #[test]
    fn zip_same_second_empty() {
        let mut x_yielded = Vec::new();
        let x = [1, 2].into_iter().map(|value| {
            x_yielded.push(value);
            value
        });
        let y = ([] as [i32; 0]).into_iter();
        let mut z = zip_same(x, y);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
        drop(z);
        assert_eq!(x_yielded.len(), 1);
        let mut x_yielded_iter = x_yielded.into_iter();
        assert_eq!(x_yielded_iter.next().unwrap(), 1);
    }

    /// Tests that the zip_same iterator does the following when no
    /// elements are in common:
    /// 1. no elements are yielded
    /// 2. the source iterators are each advanced one element
    #[test]
    fn zip_same_none_in_common() {
        let mut x_yielded: Vec<i32> = Vec::new();
        let mut y_yielded: Vec<i32> = Vec::new();
        let x = [1, 2, 3, 4].into_iter().map(|value| {
            x_yielded.push(value);
            value
        });
        let y = vec![5, 6, 7, 8].into_iter().map(|value| {
            y_yielded.push(value);
            value
        });
        let mut z = zip_same(x, y);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
        drop(z);
        assert_eq!(x_yielded.len(), 1);
        let mut x_yielded_iter = x_yielded.into_iter();
        assert_eq!(x_yielded_iter.next().unwrap(), 1);
        assert_eq!(y_yielded.len(), 1);
        let mut y_yielded_iter = y_yielded.into_iter();
        assert_eq!(y_yielded_iter.next().unwrap(), 5);
    }

    /// Tests that the zip_same iterator:
    /// 1. yields identical elements
    /// 2. yields None forever after being depleted
    /// 3. does not yield from the source iterators more than it needs to
    #[test]
    fn zip_same_both_continue() {
        let mut x_yielded: Vec<i32> = Vec::new();
        let mut y_yielded: Vec<i32> = Vec::new();
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

    /// Tests that the zip_same iterator stops yielding elements
    /// from the iterators it is passed if one of them ends.
    #[test]
    fn zip_same_first_continues() {
        let mut x_yielded: Vec<i32> = Vec::new();
        let mut y_yielded: Vec<i32> = Vec::new();
        let x = [1, 2, 3, 4].into_iter().map(|value| {
            x_yielded.push(value);
            value
        });
        let y = vec![1].into_iter().map(|value| {
            y_yielded.push(value);
            value
        });
        let mut z = zip_same(x, y);
        assert_eq!(z.next().unwrap(), 1);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
        drop(z);
        assert_eq!(x_yielded.len(), 2);
        let mut x_yielded_iter = x_yielded.into_iter();
        assert_eq!(x_yielded_iter.next().unwrap(), 1);
        assert_eq!(x_yielded_iter.next().unwrap(), 2);
        assert_eq!(y_yielded.len(), 1);
        let mut y_yielded_iter = y_yielded.into_iter();
        assert_eq!(y_yielded_iter.next().unwrap(), 1);
    }

    /// Tests that the zip_same iterator stops yielding elements
    /// from the iterators it is passed if one of them ends.
    #[test]
    fn zip_same_second_continues() {
        let mut x_yielded: Vec<i32> = Vec::new();
        let mut y_yielded: Vec<i32> = Vec::new();
        let x = [1].into_iter().map(|value| {
            x_yielded.push(value);
            value
        });
        let y = vec![1, 2, 5, 4, 6].into_iter().map(|value| {
            y_yielded.push(value);
            value
        });
        let mut z = zip_same(x, y);
        assert_eq!(z.next().unwrap(), 1);
        for _ in 0..5 {
            assert!(z.next().is_none());
        }
        drop(z);
        assert_eq!(x_yielded.len(), 1);
        let mut x_yielded_iter = x_yielded.into_iter();
        assert_eq!(x_yielded_iter.next().unwrap(), 1);
        assert_eq!(y_yielded.len(), 2);
        let mut y_yielded_iter = y_yielded.into_iter();
        assert_eq!(y_yielded_iter.next().unwrap(), 1);
        assert_eq!(y_yielded_iter.next().unwrap(), 2);
    }

    /// Tests the load_n_tokens function in the case where there are more
    /// tokens than expected. In this case Err(true) should be returned.
    #[test]
    fn load_n_tokens_too_many() {
        assert_pattern!(load_n_tokens("hello,world".split(","), 1), Err(true));
    }

    /// Tests the load_n_tokens function in the case where there are fewer
    /// tokens than expected. In this case Err(false) should be returned.
    #[test]
    fn load_n_tokens_not_enough() {
        assert_pattern!(
            load_n_tokens("hello,world,howdy".split(","), 4),
            Err(false)
        );
    }

    /// Tests the load_n_tokens function in the case where there are more
    /// tokens than expected. In this case Err(true) should be returned.
    #[test]
    fn load_n_tokens_correct_number() {
        let tokens =
            load_n_tokens("hello,world,my,name,is,,Keith".split(','), 7)
                .unwrap();
        let mut tokens = tokens.into_iter();
        assert_eq!(*tokens.next().unwrap(), "hello");
        assert_eq!(*tokens.next().unwrap(), "world");
        assert_eq!(*tokens.next().unwrap(), "my");
        assert_eq!(*tokens.next().unwrap(), "name");
        assert_eq!(*tokens.next().unwrap(), "is");
        assert_eq!(*tokens.next().unwrap(), "");
        assert_eq!(*tokens.next().unwrap(), "Keith");
        assert!(tokens.next().is_none());
    }

    /// Ensures that TempFile::with_contents creates a file with the given contents.
    #[test]
    fn temp_file_with_contents() {
        let path = temp_dir().join("temp_file_with_contents.txt");
        let temp_file =
            TempFile::with_contents(path.clone(), "hello, world").unwrap();
        assert_eq!(
            fs::read_to_string(&temp_file.path).unwrap().as_str(),
            "hello, world"
        );
        drop(temp_file);
        assert!(!fs::exists(path).unwrap());
    }

    /// Checks that current() is always empty when IteratorWithMemory
    /// is given an iterator that doesn't yield any elements.
    #[test]
    fn iterator_with_memory_empty() {
        let v: Vec<i32> = Vec::new();
        let mut i = IteratorWithMemory::new(v.into_iter());
        assert!(i.current().is_none());
        i.advance();
        assert!(i.current().is_none());
    }

    /// Checks that the IteratorWithMemory::advance() method
    /// advances the iterator by exactly one element each time.
    #[test]
    fn iterator_with_memory_advance_manually() {
        let mut i = IteratorWithMemory::new(vec![1, 3, 5, 4].into_iter());
        assert!(i.current().is_none());
        i.advance();
        assert_eq!(i.current().unwrap(), &1);
        i.advance();
        assert_eq!(i.current().unwrap(), &3);
        i.advance();
        assert_eq!(i.current().unwrap(), &5);
        i.advance();
        assert_eq!(i.current().unwrap(), &4);
        i.advance();
        assert!(i.current().is_none());
        i.advance();
        assert!(i.current().is_none());
    }

    /// Checks that the IteratorWithMemory::advance_until method zero or
    /// more elements and either the current() is either None or Some(x)
    /// where x satisfies the condition given to the method
    #[test]
    fn iterator_with_memory_advance_with_condition() {
        let mut i = IteratorWithMemory::new(vec![1, 3, 5, 4].into_iter());
        i.advance_until(|element| element % 2 == 0);
        assert_eq!(i.current().unwrap(), &4);
        i.advance_until(|element| element % 2 == 0);
        assert_eq!(i.current().unwrap(), &4);
        i.advance();
        assert!(i.current().is_none());
        i.advance_until(|element| element % 2 == 0);
        assert!(i.current().is_none());
    }
}
