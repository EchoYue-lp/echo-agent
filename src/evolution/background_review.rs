//! Background review entrypoint for memory evolution.
//!
//! The implementation currently lives in `improve` for backward compatibility,
//! but the public evolution-facing API is exported from this module.

pub use crate::improve::{BackgroundReviewConfig, BackgroundReviewer, ReviewOutcome};
