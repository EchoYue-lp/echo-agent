//! Compatibility re-export for background review.
//!
//! The implementation lives in `crate::evolution::background_review`. Keep this
//! module so existing `echo_agent::improve::BackgroundReviewer` imports continue
//! to compile while new code uses `echo_agent::evolution`.

pub use crate::evolution::background_review::{
    BackgroundReviewConfig, BackgroundReviewer, ReviewOutcome,
};
