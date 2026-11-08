#![cfg(all(feature = "files", feature = "testing"))]

// 诊断：只复用当前冻结Frame实际库，比较reader页、pipeline结果与下一模型请求。
use echo_agent::agent::{
    Agent, AgentEvent, AgentInvocationContext, CancellationToken, ReactAgentBuilder,
};
use echo_agent::testing::MockLlmClient;
use echo_agent::tools::artifact::{
    ToolOutputArtifactConfig, ToolOutputArtifactIdentity, persist_tool_output,
};
use echo_agent::tools::files::artifact::{ArtifactPageLimit, read_artifact_page};
use futures::StreamExt;
use std::sync::Arc;

#[test]
fn unconsumed_artifact_pages_reach_model_across_compaction()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let root = std::env::var_os("ECHO_TOOL_PAGE_PROBE_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| temporary.path().to_path_buf());
    std::fs::create_dir_all(&root)?;
    let content = format!("{}TAIL", "早期仍有效的完整背景 ".repeat(2_000));
    let config = ToolOutputArtifactConfig::new(&root, "diagnostic").threshold_bytes(32 * 1024);
    let artifact = persist_tool_output(
        config.clone().threshold_bytes(1),
        ToolOutputArtifactIdentity {
            conversation_id: Some("probe".to_string()),
            run_id: Some("probe".to_string()),
            call_id: "original".to_string(),
            tool_name: "source".to_string(),
        },
        &content,
    )?
    .ok_or_else(|| echo_agent::error::ReactError::Other("missing artifact".to_string()))?;
    let mut cursor = None;
    let mut script = MockLlmClient::new();
    let mut pages = Vec::new();
    for ordinal in 0..31 {
        let id = format!("probe-read-{ordinal}");
        let mut args = serde_json::json!({"path": artifact.path, "expected_sha256":artifact.sha256, "max_tokens":2000});
        if let Some(cursor) = cursor.as_ref() {
            args.as_object_mut()
                .ok_or_else(|| echo_agent::error::ReactError::Other("not object".to_string()))?
                .insert("cursor".into(), serde_json::json!(cursor));
        }
        script = script.then_tool_call(&id, "read_artifact", args.to_string());
        let page = read_artifact_page(
            &config,
            &artifact,
            cursor.as_deref(),
            ArtifactPageLimit::Tokens(2000),
        )?;
        let done = page.complete;
        cursor = page.next_cursor;
        pages.push((id, page.content));
        if done {
            break;
        }
    }
    let model = Arc::new(script.with_response("done"));
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        let agent = ReactAgentBuilder::new().enable_tools().auto_project_rules(false)
            .model("mock-model").llm_client(model.clone()).tool_output_artifacts(config).token_limit(8000)
            .max_iterations(32).system_prompt("diagnostic system background ".repeat(40)).build()?;
        agent.context().lock().await.set_compressor(
            echo_agent::compression::compressor::SlidingWindowCompressor::new(20).with_recent_token_budget(400));
        let mut stream = agent.execute_stream_message_with_invocation_context(
            echo_agent::llm::types::Message::user("读取原文".to_string()),
            CancellationToken::new(), AgentInvocationContext {
                allowed_tools: Some(["read_artifact".to_string()].into_iter().collect()),
                runtime: Some(echo_agent::tools::ExternalRunContext {
                    run_id: Some("probe".to_string()), conversation_id: Some("probe".to_string()),
                    turn_id: Some("probe-turn".to_string()), execution_id: Some("probe-turn".to_string()),
                    ..Default::default()
                }), ..Default::default()
            }).await?;
        let mut results = Vec::new();
        let mut compressed = 0usize;
        let mut notes = Vec::new();
        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::ToolResult {call_id,name,result} => results.push(serde_json::json!({"call_id":call_id,"name":name,"result":result})),
                AgentEvent::ContextCompressed {..} => compressed = compressed.saturating_add(1),
                other => notes.push(format!("{other:?}")),
            }
        }
        let calls = model.all_calls();
        let mut checked = Vec::new();
        for (id, page) in &pages {
            let actual = calls.iter().flat_map(|messages| messages.iter()).filter(|m|m.tool_call_id.as_deref()==Some(id.as_str()))
                .filter_map(|m|m.content.as_text()).collect::<Vec<_>>();
            checked.push(serde_json::json!({"call_id":id,"expected_page":page,"actual_tool_messages":actual}));
        }
        let data = serde_json::json!({"model_calls":calls.len(),"compressed":compressed,"other_events":notes,"source":artifact,"tool_results":results,"checked":checked,"all_model_requests":calls});
        std::fs::write(root.join("actual-results.json"),serde_json::to_vec_pretty(&data).map_err(|e|echo_agent::error::ReactError::Other(e.to_string()))?)
            .map_err(|e|echo_agent::error::ReactError::Other(e.to_string()))?;
        println!("MODEL_CALLS {} PAGES {} COMPRESSED {}",calls.len(),pages.len(),compressed);
        let mut pipeline_source = String::new();
        let mut model_source = String::new();
        let mut missing = Vec::new();
        let mut expected_start = 0_u64;
        for (id, expected) in &pages {
            let row = results.iter().find(|row| row.get("call_id").and_then(serde_json::Value::as_str)==Some(id.as_str())).ok_or_else(||echo_agent::error::ReactError::Other("missing result event".to_string()))?;
            let result = row.get("result").ok_or_else(||echo_agent::error::ReactError::Other("missing typed result".to_string()))?;
            if result.get("success").and_then(serde_json::Value::as_bool)!=Some(true) {return Err(echo_agent::error::ReactError::Other(format!("reader failed: {id}")));}
            let output = result.get("output").and_then(serde_json::Value::as_str).ok_or_else(||echo_agent::error::ReactError::Other("result lacks output".to_string()))?;
            let metadata = result.get("metadata").ok_or_else(||echo_agent::error::ReactError::Other("result lacks metadata".to_string()))?;
            let number = |key:&str| -> Result<u64,echo_agent::error::ReactError> {
                metadata.get(key).and_then(serde_json::Value::as_str).ok_or_else(||echo_agent::error::ReactError::Other("missing range".to_string()))?.parse::<u64>().map_err(|e|echo_agent::error::ReactError::Other(e.to_string()))
            };
            let start=number("start_byte")?; let end=number("end_byte")?; let total=number("total_bytes")?;
            if start!=expected_start || end<start || total!=artifact.artifact_bytes {return Err(echo_agent::error::ReactError::Other(format!("range mismatch: {id}")));}
            let suffix=if let Some(next)=metadata.get("next_cursor").and_then(serde_json::Value::as_str) {
                format!("\n\n[Artifact page: bytes {start}-{end} of {total}; continue with cursor={next}.]")
            } else {format!("\n\n[Artifact complete: bytes {start}-{end} of {total}.]")};
            let body=output.strip_suffix(&suffix).ok_or_else(||echo_agent::error::ReactError::Other("invalid original page envelope".to_string()))?;
            // 范围/元数据的bytes是UTF8字节合同，len不参与字符截断。
            if body!=expected || u64::try_from(body.len()).map_err(|e|echo_agent::error::ReactError::Other(e.to_string()))?!=end.saturating_sub(start) {return Err(echo_agent::error::ReactError::Other(format!("full UTF8 page mismatch: {id}")));}
            pipeline_source.push_str(body); expected_start=end;
            let actual=calls.iter().flat_map(|messages|messages.iter()).filter(|m|m.tool_call_id.as_deref()==Some(id.as_str())).filter_map(|m|m.content.as_text()).find(|text|text==output);
            if let Some(actual)=actual {model_source.push_str(actual.strip_suffix(&suffix).ok_or_else(||echo_agent::error::ReactError::Other("model envelope mismatch".to_string()))?);} else {missing.push(id.clone());}
        }
        if pipeline_source!=content || expected_start!=artifact.artifact_bytes {return Err(echo_agent::error::ReactError::Other("pipeline source reconstruction failed".into()));}
        if !missing.is_empty() {return Err(echo_agent::error::ReactError::Other(format!("actual reader/pipeline retained full source, but model never received pages {missing:?}; reconstructed model bytes={}, source bytes={}",model_source.len(),content.len())));}
        if model_source!=content {return Err(echo_agent::error::ReactError::Other("model source reconstruction mismatch".into()));}

        Ok::<(), echo_agent::error::ReactError>(())
    })?;
    Ok(())
}
