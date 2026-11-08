// Included inside the existing framework pipeline tests: no public test API.
mod harness {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/performance_baseline.rs"
    ));
}
use crate::tokenizer::{HeuristicTokenizer, Tokenizer};
use crate::tools::artifact::ToolOutputArtifactConfig;
use crate::tools::files::artifact::{ArtifactPageLimit, ArtifactReadErrorKind, read_artifact_page};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Deserialize)]
struct ArtifactFacts {
    root_dir: PathBuf,
    retention: String,
    threshold_bytes: usize,
    max_age_secs: Option<u64>,
}
#[derive(Deserialize)]
struct PayloadCase {
    group: String,
    raw_result: String,
    upstream_complete: Option<String>,
}
#[derive(Deserialize)]
struct ProducerFacts {
    fixed_root: PathBuf,
    artifact_config: ArtifactFacts,
    max_tool_output_tokens: Option<usize>,
    payloads: Vec<PayloadCase>,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn config(facts: &ProducerFacts) -> ToolOutputArtifactConfig {
    ToolOutputArtifactConfig::new(
        &facts.artifact_config.root_dir,
        &facts.artifact_config.retention,
    )
    .threshold_bytes(facts.artifact_config.threshold_bytes)
    .max_age_secs(facts.artifact_config.max_age_secs)
}

fn validate_policy_output(
    raw: &ToolResult,
    result: &ToolResult,
    preview: &str,
) -> harness::HarnessResult {
    // Typed detail preserves the real producer result; the model receives the
    // original stage's separate preview. Budget alone is not content identity.
    assert_eq!(result.success, raw.success);
    assert_eq!(result.output, raw.output);
    if let Some(reference) = &result.artifact {
        assert!(result.truncated);
        assert!(preview.contains(reference.path.to_string_lossy().as_ref()));
        assert!(preview.contains(&reference.sha256));
        let prefix = raw.output.chars().take(64).collect::<String>();
        assert!(preview.starts_with(&prefix));
    } else if !result.truncated {
        assert_eq!(preview, raw.output);
    } else if preview.is_empty() {
        return Err("truncated model preview lost all content/notice".into());
    }
    Ok(())
}

fn recover_full(
    config: &ToolOutputArtifactConfig,
    reference: &crate::tools::artifact::ToolOutputArtifactRef,
) -> harness::HarnessResult<(String, usize)> {
    let mut text = String::new();
    let mut cursor = None;
    let mut offset = 0_u64;
    let mut pages = 0_usize;
    loop {
        let page = read_artifact_page(
            config,
            reference,
            cursor.as_deref(),
            ArtifactPageLimit::Bytes(4096),
        )?;
        assert_eq!(page.sha256, reference.sha256);
        assert_eq!(page.start_byte, offset);
        if !page.complete && page.end_byte <= offset {
            return Err("artifact cursor made no progress".into());
        }
        text.push_str(&page.content);
        offset = page.end_byte;
        pages = pages.checked_add(1).ok_or("page count overflow")?;
        if page.complete {
            assert!(page.next_cursor.is_none());
            break;
        }
        cursor = Some(page.next_cursor.ok_or("incomplete page omitted cursor")?);
        if u64::try_from(pages)? > reference.artifact_bytes.saturating_add(1) {
            return Err("unbounded artifact paging".into());
        }
    }
    // 此oracle比较artifact字节数，不做文本截断；String.len与原as_bytes.len等值。
    assert_eq!(u64::try_from(text.len())?, reference.artifact_bytes);
    assert_eq!(hash(text.as_bytes()), reference.sha256);
    Ok((text, pages))
}

fn snapshot(
    root: &Path,
    facts: &ProducerFacts,
) -> harness::HarnessResult<crate::agent::snapshot::AgentRunSnapshot> {
    let budget = facts
        .max_tool_output_tokens
        .ok_or("actual EKO model-result budget missing")?;
    let agent = crate::agent::ReactAgentBuilder::new()
        .model("test-model")
        .working_dir(root)
        .conversation_id("tools-conversation")
        .max_tool_output_tokens(budget)
        .tool_output_artifacts(config(facts))
        .build()?;
    Ok(crate::agent::snapshot::AgentRunSnapshot::from_agent(&agent))
}

async fn policy(
    root: &Path,
    facts: &ProducerFacts,
    item: &PayloadCase,
    snapshot: &crate::agent::snapshot::AgentRunSnapshot,
) -> harness::HarnessResult<(ToolResult, String, bool)> {
    let raw: ToolResult = serde_json::from_slice(&std::fs::read(root.join(&item.raw_result))?)?;
    let complete_origin = !raw.truncated || raw.artifact.is_some();
    let expected = if let Some(path) = &item.upstream_complete {
        std::fs::read_to_string(root.join(path))?
    } else {
        raw.output.clone()
    };
    let mut context = completed_context(raw.output.clone());
    context.call_id = format!("seed-{}", item.group);
    context.requested_tool_name = "shell".to_string();
    context.tool_name = "shell".to_string();
    context.result = Some(raw.clone());
    TruncationStage.run(&mut context, snapshot).await?;
    let result = context
        .result
        .ok_or("original truncation stage lost typed result")?;
    let preview = context
        .output
        .ok_or("original truncation stage lost model output")?;
    validate_policy_output(&raw, &result, &preview)?;
    if let Some(budget) = facts.max_tool_output_tokens {
        assert!(HeuristicTokenizer.count_tokens(&preview) <= budget);
    }
    if let Some(reference) = &result.artifact {
        let (actual, _) = recover_full(&config(facts), reference)?;
        assert_eq!(actual, expected);
    }
    if item.group == "over" {
        assert!(complete_origin);
        assert!(result.artifact.is_some());
    }
    Ok((result, preview, complete_origin))
}

async fn prepare(root: &Path, facts: &ProducerFacts) -> harness::HarnessResult {
    let snapshot = snapshot(root, facts)?;
    std::fs::create_dir_all(root.join("tool-final"))?;
    for item in &facts.payloads {
        let (result, preview, complete_origin) = policy(root, facts, item, &snapshot).await?;
        std::fs::write(
            root.join(format!("tool-final/{}.json", item.group)),
            serde_json::to_vec_pretty(&result)?,
        )?;
        std::fs::write(
            root.join(format!("tool-final/{}.model.txt", item.group)),
            preview.as_bytes(),
        )?;
        let pages = if let Some(reference) = &result.artifact {
            let (full, pages) = recover_full(&config(facts), reference)?;
            std::fs::write(
                root.join(format!("tool-final/{}.complete.txt", item.group)),
                full.as_bytes(),
            )?;
            pages
        } else {
            0
        };
        std::fs::write(
            root.join(format!("tool-final/{}.facts.json", item.group)),
            serde_json::to_vec_pretty(&json!({
                "complete_origin":complete_origin,"artifact":result.artifact,"pages":pages,"model_preview_bytes":preview.len(),
                "model_estimated_tokens":HeuristicTokenizer.count_tokens(&preview),"stage":"original TruncationStage",
                "producer":"frozen typed result from actual EKO registered shell","scope":"component stages; not one end-to-end execution latency"}))?,
        )?;
    }
    Ok(())
}

async fn sample(root: &Path, facts: &ProducerFacts) -> harness::HarnessResult {
    let snapshot = snapshot(root, facts)?;
    let (outer_phase, outer_index) = harness::phase_index()?;
    assert_eq!(outer_phase, "measurement");
    assert_eq!(outer_index, 0);
    for item in &facts.payloads {
        let raw: ToolResult = serde_json::from_slice(&std::fs::read(root.join(&item.raw_result))?)?;
        let complete_origin = !raw.truncated || raw.artifact.is_some();
        let expected = if let Some(path) = &item.upstream_complete {
            std::fs::read_to_string(root.join(path))?
        } else {
            raw.output.clone()
        };
        for ordinal in 0_usize..35 {
            let mut context = completed_context(raw.output.clone());
            context.call_id = format!("seed-{}", item.group);
            context.tool_name = "shell".to_string();
            context.result = Some(raw.clone());
            let started = Instant::now();
            TruncationStage.run(&mut context, &snapshot).await?;
            let policy_ns = started.elapsed().as_nanos();
            let result = context.result.ok_or("typed result missing")?;
            let preview = context.output.ok_or("model output missing")?;
            validate_policy_output(&raw, &result, &preview)?;
            if item.group == "over" {
                assert!(complete_origin);
                assert!(result.artifact.is_some());
            }
            let (phase, index) = if ordinal < 5 {
                ("warmup", ordinal)
            } else {
                ("measurement", ordinal - 5)
            };
            if let Some(budget) = facts.max_tool_output_tokens {
                assert!(HeuristicTokenizer.count_tokens(&preview) <= budget);
            }
            harness::emit(
                &format!("tool/original-truncation-policy/{}", item.group),
                "hot",
                phase,
                index,
                policy_ns,
                if complete_origin {
                    "success"
                } else {
                    "preview_only"
                },
                json!({"ok":true,"preview_bytes":preview.len(),
                    "estimated_tokens":HeuristicTokenizer.count_tokens(&preview),"artifact":result.artifact,"upstream_truncated":raw.truncated,
                    "complete_origin":complete_origin,"scope":"original stage over frozen actual EKO raw result; setup/validation excluded"}),
            )?;
            if let Some(reference) = &result.artifact {
                let started = Instant::now();
                let (full, pages) = recover_full(&config(facts), reference)?;
                let elapsed = started.elapsed().as_nanos();
                assert_eq!(full, expected);
                harness::emit(
                    &format!("tool/full-verified-artifact-pagination/{}", item.group),
                    "hot",
                    phase,
                    index,
                    elapsed,
                    if complete_origin {
                        "success"
                    } else {
                        "preview_only"
                    },
                    json!({"ok":true,"pages":pages,"artifact_bytes":reference.artifact_bytes,
                        "payload_bytes":reference.payload_bytes,"sha256":reference.sha256,"complete_origin":complete_origin,
                        "scope":"original reader pages plus UTF8 accumulation and independent length/SHA checks; expected text equality after timer; logical bytes are not physical IO"}),
                )?;
            }
        }
    }
    let item = facts
        .payloads
        .iter()
        .find(|item| item.group == "over")
        .ok_or("over-threshold case missing")?;
    let result: ToolResult = serde_json::from_slice(&std::fs::read(
        root.join(format!("tool-final/{}.json", item.group)),
    )?)?;
    let reference = result
        .artifact
        .ok_or("positive full-artifact control absent")?;
    let started = Instant::now();
    let clamped = read_artifact_page(
        &config(facts),
        &reference,
        None,
        ArtifactPageLimit::Tokens(1_000_000),
    )?;
    let elapsed = started.elapsed().as_nanos();
    assert!(HeuristicTokenizer.count_tokens(&clamped.content) <= 3500);
    harness::emit(
        "tool/artifact-token-limit-clamp",
        "diagnostic",
        "measurement",
        0,
        elapsed,
        "success",
        json!({"ok":true,
        "requested_tokens":1_000_000,"existing_max_tokens":3500,"actual_estimated_tokens":HeuristicTokenizer.count_tokens(&clamped.content)}),
    )?;
    let started = Instant::now();
    let rejected = read_artifact_page(
        &config(facts),
        &reference,
        Some("invalid-cursor"),
        ArtifactPageLimit::Bytes(4096),
    );
    let elapsed = started.elapsed().as_nanos();
    let error = rejected.err().ok_or("invalid cursor accepted")?;
    assert_eq!(error.kind(), ArtifactReadErrorKind::InvalidCursor);
    harness::emit(
        "tool/artifact-invalid-cursor-refusal",
        "diagnostic",
        "measurement",
        0,
        elapsed,
        "expected_refusal",
        json!({"ok":true,"not_complete_output":true}),
    )
}

#[tokio::test]
#[ignore = "retained files-feature release harness; explicit coordinator measurement window"]
async fn performance_baseline_tool_output() -> harness::HarnessResult {
    if cfg!(debug_assertions) {
        return Err("release harness required".into());
    }
    let root = harness::root()?;
    let facts: ProducerFacts = serde_json::from_slice(&std::fs::read(root.join("fixture.json"))?)?;
    assert_eq!(facts.fixed_root, root);
    assert!(facts.artifact_config.root_dir.starts_with(&root));
    match harness::required("EKO_PERF_ACTION")?.as_str() {
        "prepare" => prepare(&root, &facts).await,
        "validate" => {
            for item in &facts.payloads {
                let result: ToolResult = serde_json::from_slice(&std::fs::read(
                    root.join(format!("tool-final/{}.json", item.group)),
                )?)?;
                if let Some(reference) = &result.artifact {
                    recover_full(&config(&facts), reference)?;
                }
            }
            Ok(())
        }
        "sample" => sample(&root, &facts).await,
        _ => {
            Err("tool policy requires existing producer seed, then prepare/validate/sample".into())
        }
    }
}
