#![cfg(unix)]

use echo_execution::skills::hooks::{
    HookAction, HookContext, HookEvent, HookRegistry, HookResult, HookRule, HooksDefinition,
};
use serde_json::json;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn fixture_directory(label: &str) -> TestResult<PathBuf> {
    // 测试工件留在调用方指定的主根内；不借用户 HOME 或系统临时目录。
    let root = match std::env::var_os("ECHO_HOOK_DEADLINE_TEST_ROOT") {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir()?.join(".echo-semantic/local/hook-command-deadline-tests"),
    };
    if !root.is_absolute() {
        return Err("Hook 测试工件根必须是绝对路径".into());
    }
    let directory = root.join(format!("{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn registry(directory: &Path, command: &str, timeout: u64) -> TestResult<HookRegistry> {
    let action = HookAction::Command {
        command: command.to_string(),
        shell: None,
        timeout,
    };
    action.validate().map_err(std::io::Error::other)?;
    let mut definition = HooksDefinition::default();
    definition.add_rules(
        HookEvent::PreToolUse,
        vec![HookRule {
            matcher: "Read".to_string(),
            hooks: vec![action],
        }],
    );
    let mut registry = HookRegistry::new();
    registry.register("command-deadline", &directory.to_string_lossy(), definition);
    Ok(registry)
}

fn pid(directory: &Path) -> TestResult<i32> {
    let captured = std::fs::read_to_string(directory.join("original-child.pid"))?;
    let pid = captured.trim().parse::<i32>()?;
    if pid <= 0 {
        return Err("原 child 未发布有效正 PID".into());
    }
    Ok(pid)
}

fn process_exists(pid: i32) -> TestResult<bool> {
    // SAFETY: signal 0 只检查本 fixture 发布的正 PID，不发送终止信号、不读进程参数。
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error.into()),
    }
}

async fn observe_original_child_absent(directory: &Path) -> TestResult {
    let original = pid(directory)?;
    // 这是测试 harness 的遗留进程反控，独立于被测 Command.timeout；
    // 不终止子进程、不把杀进程或测试 future 被取消当作 Hook 已经正确结算。
    tokio::time::timeout(Duration::from_secs(2), async {
        while process_exists(original)? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Box<dyn Error + Send + Sync>>(())
    })
    .await??;
    Ok(())
}

async fn call(
    directory: &Path,
    command: &str,
    timeout: u64,
    payload: serde_json::Value,
) -> TestResult<HookResult> {
    let registry = registry(directory, command, timeout)?;
    let observed = tokio::time::timeout(
        Duration::from_secs(8),
        registry.run_pre_tool_use("Read", &payload, "command-deadline-fixture"),
    )
    .await;
    // RED 双向死锁也须等原 PID 消失；原实现的 kill_on_drop 只作为测试救援，
    // 不能因此给被测1秒deadline或cleanup能力签票。
    observe_original_child_absent(directory).await?;
    match observed {
        Ok(result) => Ok(result),
        Err(_) => Err("外层8秒测试 guard 已到，被测 Hook 未完成".into()),
    }
}

#[tokio::test]
async fn command_timeout_covers_large_stdin_and_reaps_original_child() -> TestResult {
    let directory = fixture_directory("large-input-deadline")?;
    let started = Instant::now();
    // 命令合法且不读 stdin，4秒有限后继用于让旧实现自然产生晚成功，
    // 不用无穷进程把原 RED 变成不可收口的运行。
    let command = r#"printf '%s\n' "$$" > original-child.pid; sleep 4; printf late > late-effect; printf '{"systemMessage":"late-success"}'"#;
    let result = call(
        &directory,
        command,
        1,
        json!({"large": "合法🙂".repeat(400_000)}),
    )
    .await?;
    eprintln!(
        "HOOK_PUBLIC_DEADLINE_RECEIPT timeout_secs=1 total_observation_ms={} original_pid={} child_absent=true late_effect={}",
        started.elapsed().as_millis(),
        pid(&directory)?,
        directory.join("late-effect").exists(),
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| message == "Hook command timed out after 1s"),
        "未观察到原1秒动作超时：{result:?}"
    );
    assert!(!directory.join("late-effect").exists());
    assert!(
        !result
            .messages
            .iter()
            .any(|message| message == "late-success")
    );
    assert!(result.injected_context.is_none());
    Ok(())
}

#[tokio::test]
async fn command_duplex_drains_large_stdout_stderr_before_reading_stdin() -> TestResult {
    let directory = fixture_directory("duplex-output-before-input")?;
    // bash 内建先写两个超过常见pipe容量的合法空白前缀，再读完整JSON。
    // stdout.trim()仍是同一合法Hook JSON，不改变既有输出合同。
    let command = r#"printf '%s\n' "$$" > original-child.pid; for ((i=0;i<128;i++)); do printf '%2048s' ' '; printf '%2048s' ' ' >&2; done; IFS= read -r hook_json; case "$hook_json" in *'"hook_event_name":"PreToolUse"'*) ;; *) exit 9 ;; esac; case "$hook_json" in *'tail-marker'*) printf '{"systemMessage":"duplex-completed"}' ;; *) exit 9 ;; esac"#;
    let result = call(
        &directory,
        command,
        3,
        json!({"large": "x".repeat(2 * 1024 * 1024), "tail": "tail-marker"}),
    )
    .await?;
    assert_eq!(result.messages, vec!["duplex-completed".to_string()]);
    assert!(!result.block);
    Ok(())
}

#[tokio::test]
async fn command_large_stdin_early_exit_keeps_legal_output() -> TestResult {
    let directory = fixture_directory("early-exit-legal-output")?;
    let result = call(
        &directory,
        r#"printf '%s\n' "$$" > original-child.pid; printf '{"systemMessage":"early-success"}'"#,
        3,
        json!({"large": "x".repeat(2 * 1024 * 1024)}),
    )
    .await?;
    assert_eq!(result.messages, vec!["early-success".to_string()]);
    assert!(!result.block);
    Ok(())
}

#[tokio::test]
async fn command_exit_two_still_overrides_structured_allow() -> TestResult {
    let directory = fixture_directory("exit-two-block")?;
    let result = call(
        &directory,
        r#"printf '%s\n' "$$" > original-child.pid; printf '{"decision":"allow","systemMessage":"block-control"}'; printf original-block >&2; exit 2"#,
        3,
        json!({"large": "x".repeat(2 * 1024 * 1024)}),
    )
    .await?;
    assert!(result.block);
    assert_eq!(result.block_reason.as_deref(), Some("original-block"));
    assert!(
        result
            .messages
            .iter()
            .any(|message| message == "block-control")
    );
    Ok(())
}

#[tokio::test]
async fn command_large_input_keeps_public_hook_context_and_stdin_eof() -> TestResult {
    let directory = fixture_directory("context-json-eof")?;
    let command = r#"printf '%s\n' "$$" > original-child.pid; IFS= read -r hook_json; if IFS= read -r extra; then exit 8; fi; case "$hook_json" in *'"hook_event_name":"PreToolUse"'*) ;; *) exit 9 ;; esac; case "$hook_json" in *'context-tail'*) printf '{"hookSpecificOutput":{"permissionDecision":"ask"}}' ;; *) exit 9 ;; esac"#;
    let action = registry(&directory, command, 3)?;
    let payload = json!({"large": "x".repeat(2 * 1024 * 1024), "tail": "context-tail"});
    let context = HookContext::for_pre_tool_use("Read", &payload, "context-fixture", "");
    let result =
        tokio::time::timeout(Duration::from_secs(8), action.run_lifecycle_hooks(&context)).await?;
    observe_original_child_absent(&directory).await?;
    assert!(matches!(
        result.permission_decision,
        Some(echo_core::tools::permission::PermissionDecision::Ask { .. })
    ));
    assert!(!result.block);
    Ok(())
}
