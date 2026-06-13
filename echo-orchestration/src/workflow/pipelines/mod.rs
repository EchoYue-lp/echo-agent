//! Pre-built pipeline workflows
//!
//! Ready-to-use graph workflows for common multi-step agent tasks:
//!
//! | Pipeline | Stages | Description |
//! |----------|--------|-------------|
//! | [`data_pipeline`] | load_data -> profile -> analyze -> visualize -> summarize | End-to-end data analysis |
//! | [`writing_pipeline`] | outline -> draft -> review -> revise (loop) -> finalize | Content creation with quality loop |

pub mod data_pipeline;
pub mod writing_pipeline;

pub use data_pipeline::{DataPipelineConfig, run_data_pipeline};
pub use writing_pipeline::{WritingPipelineConfig, run_writing_pipeline};
