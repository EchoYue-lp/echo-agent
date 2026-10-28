// Test-only adapter for the cross-repository measurement log.
use serde_json::{Value, json};
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;

pub(super) type HarnessResult<T = ()> = Result<T, Box<dyn Error>>;
pub(super) fn required(name: &str) -> HarnessResult<String> {
    let value = std::env::var(name)?;
    if value.trim().is_empty() {
        return Err(format!("{name} is required").into());
    }
    Ok(value)
}
pub(super) fn root() -> HarnessResult<PathBuf> {
    let root = PathBuf::from(required("EKO_PERF_ROOT")?);
    if !root.is_absolute()
        || root.as_path() != std::path::Path::new(&required("EKO_PERF_FIXED_ROOT")?)
    {
        return Err("fixture absolute root differs from frozen authority".into());
    }
    Ok(root)
}
pub(super) fn phase_index() -> HarnessResult<(String, usize)> {
    let phase = required("EKO_PERF_PHASE")?;
    if !["warmup", "measurement"].contains(&phase.as_str()) {
        return Err("invalid phase".into());
    }
    Ok((phase, required("EKO_PERF_SAMPLE_INDEX")?.parse()?))
}
pub(super) fn emit(
    scenario: &str,
    mode: &str,
    phase: &str,
    index: usize,
    elapsed_ns: u128,
    outcome: &str,
    correctness: Value,
) -> HarnessResult {
    let row = json!({"scenario":scenario,"mode":mode,"phase":phase,"sample_index":index,
        "elapsed_ns":elapsed_ns.to_string(),"outcome":outcome,
        "dataset_sha256":required("EKO_PERF_DATASET_SHA256")?,"source_sha256":required("EKO_PERF_SOURCE_SHA256")?,
        "protocol_sha256":required("EKO_PERF_PROTOCOL_SHA256")?,"environment_id":required("EKO_PERF_ENVIRONMENT_ID")?,
        "process_observation_id":required("EKO_PERF_PROCESS_ID")?,"correctness":correctness});
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(required("EKO_PERF_SAMPLES")?)?;
    writeln!(file, "{row}")?;
    Ok(())
}
