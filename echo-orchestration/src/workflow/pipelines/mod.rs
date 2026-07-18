//! Pre-built pipeline workflows
//!
//! Ready-to-use graph workflows for common Agent tasks:
//!
//! | Pipeline | Stages | Description |
//! |----------|--------|-------------|
//! | [`data_pipeline`] | inspect -> persist script -> execute -> verify artifacts | Code-first reproducible data analysis |
//! | [`writing_pipeline`] | outline -> draft -> review -> revise (loop) -> finalize | Content creation with quality loop |

pub mod data_pipeline;
pub mod writing_pipeline;

pub use data_pipeline::{DataPipelineConfig, DataPipelineLanguage, run_data_pipeline};
pub use writing_pipeline::{WritingPipelineConfig, run_writing_pipeline};
