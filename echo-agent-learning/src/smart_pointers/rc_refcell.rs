//! `Rc<T>` and `RefCell<T>` for single-threaded shared ownership.

use crate::errors::LearningError;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
pub struct SharedNotebook {
    entries: Rc<RefCell<Vec<String>>>,
}

impl SharedNotebook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, entry: impl Into<String>) -> Result<(), LearningError> {
        self.entries
            .try_borrow_mut()
            .map_err(|_| LearningError::BorrowConflict)?
            .push(entry.into());
        Ok(())
    }

    pub fn entries(&self) -> Result<Vec<String>, LearningError> {
        self.entries
            .try_borrow()
            .map(|entries| entries.clone())
            .map_err(|_| LearningError::BorrowConflict)
    }

    pub fn strong_count(&self) -> usize {
        Rc::strong_count(&self.entries)
    }

    /// Keep a read guard alive while attempting a write, producing a handled
    /// runtime borrow conflict instead of a panic.
    pub fn demonstrate_conflict(&self) -> Result<(), LearningError> {
        let _reading = self
            .entries
            .try_borrow()
            .map_err(|_| LearningError::BorrowConflict)?;
        self.add("this write conflicts with the active read")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_same_notebook() -> Result<(), LearningError> {
        let first = SharedNotebook::new();
        let second = first.clone();
        second.add("shared entry")?;
        assert_eq!(first.entries()?, vec!["shared entry"]);
        assert_eq!(first.strong_count(), 2);
        Ok(())
    }

    #[test]
    fn runtime_borrow_conflict_is_returned_as_an_error() {
        let notebook = SharedNotebook::new();
        assert_eq!(
            notebook.demonstrate_conflict(),
            Err(LearningError::BorrowConflict)
        );
    }
}
