//! `Box<T>` for recursive values and `Box<dyn Trait>` for dynamic dispatch.

use crate::traits::{PlainFormatter, TaskFormatter};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanNode {
    Step(String),
    Sequence(Box<PlanNode>, Box<PlanNode>),
}

impl PlanNode {
    pub fn node_count(&self) -> usize {
        match self {
            Self::Step(_) => 1,
            Self::Sequence(left, right) => 1usize
                .saturating_add(left.node_count())
                .saturating_add(right.node_count()),
        }
    }
}

pub fn boxed_formatter() -> Box<dyn TaskFormatter> {
    Box::new(PlainFormatter)
}

/// A small newtype that demonstrates dereference coercion to `str`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptBox(Box<str>);

impl PromptBox {
    pub fn new(prompt: impl Into<Box<str>>) -> Self {
        Self(prompt.into())
    }
}

impl Deref for PromptBox {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The counter makes deterministic RAII cleanup visible in tests and examples.
#[derive(Debug)]
pub struct DropCounter {
    dropped: Arc<AtomicUsize>,
}

impl DropCounter {
    pub fn new(dropped: Arc<AtomicUsize>) -> Self {
        Self { dropped }
    }
}

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_gives_recursive_enum_a_known_size() {
        let plan = PlanNode::Sequence(
            Box::new(PlanNode::Step("research".to_string())),
            Box::new(PlanNode::Step("write".to_string())),
        );
        assert_eq!(plan.node_count(), 3);
    }

    #[test]
    fn deref_coercion_and_drop_are_observable() {
        let prompt = PromptBox::new("hello");
        assert_eq!(prompt.len(), 5);

        let dropped = Arc::new(AtomicUsize::new(0));
        {
            let _guard = DropCounter::new(Arc::clone(&dropped));
            assert_eq!(dropped.load(Ordering::SeqCst), 0);
        }
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
