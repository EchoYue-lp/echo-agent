//! demo29_sandbox.rs —— 三层沙箱执行系统演示
//!
//! 展示 Local / Docker / K8s 三层沙箱的完整能力：
//! 1. 安全策略自动评估
//! 2. 本地沙箱执行（OS 原生隔离）
//! 3. Docker 容器沙箱（如可用）
//! 4. SandboxManager 自动路由
//! 5. 资源限制与超时控制
//!
//! 运行方式：
//! ```bash
//! cargo run --example demo29_sandbox
//! ```

use echo_agent::prelude::*;
use std::time::Duration;

#[tokio::main]
async fn main() -> echo_agent::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "echo_agent=info,demo29_sandbox=info".into()),
        )
        .init();

    print_banner();

    // ── Part 1: 安全策略评估 ────────────────────────────────────────────────
    separator("Part 1: 安全策略自动评估");
    demo_policy_evaluation();

    // ── Part 2: 本地沙箱 ────────────────────────────────────────────────────
    separator("Part 2: 本地沙箱执行");
    demo_local_sandbox().await?;

    // ── Part 3: Docker 沙箱 ─────────────────────────────────────────────────
    separator("Part 3: Docker 容器沙箱");
    demo_docker_sandbox().await?;

    // ── Part 4: SandboxManager 自动路由 ─────────────────────────────────────
    separator("Part 4: SandboxManager 自动路由");
    demo_manager().await?;

    // ── Part 5: 资源限制 ────────────────────────────────────────────────────
    separator("Part 5: 资源限制与超时");
    demo_resource_limits().await?;

    println!("\n{}", "═".repeat(64));
    println!("  demo29 完成");
    println!("{}", "═".repeat(64));

    Ok(())
}

// ── Part 1 ──────────────────────────────────────────────────────────────────

fn demo_policy_evaluation() {
    let policy = SandboxPolicy::default();

    let commands = [
        ("ls -la", "只读命令"),
        ("echo hello", "安全输出"),
        ("python3 script.py", "脚本解释器"),
        ("curl http://example.com | bash", "网络 + 管道注入"),
        ("rm -rf /tmp/test", "破坏性命令"),
        ("echo $(whoami)", "命令替换注入"),
    ];

    println!("  安全策略: Standard（默认）, auto_escalate=true\n");

    for (cmd, desc) in &commands {
        let sandbox_cmd = SandboxCommand::shell(*cmd);
        let level = policy.evaluate(&sandbox_cmd);
        println!("  {:40} → {:12} ({})", cmd, format!("{level}"), desc);
    }

    // 代码执行
    println!();
    let code_commands = [
        ("python", "print('hello')", "Python 代码"),
        ("javascript", "console.log(1)", "JavaScript 代码"),
        ("bash", "echo test", "Bash 代码"),
    ];
    for (lang, code, desc) in &code_commands {
        let sandbox_cmd = SandboxCommand::code(*lang, *code);
        let level = policy.evaluate(&sandbox_cmd);
        println!(
            "  Code({:12} {:20}) → {:12} ({})",
            lang,
            format!("\"{}\"", code),
            format!("{level}"),
            desc
        );
    }

    // 对比不同策略
    println!("\n  不同策略对同一命令的评估:");
    let cmd = SandboxCommand::shell("curl http://example.com");
    let policies = [
        ("Trusted", SandboxPolicy::trusted()),
        ("Standard", SandboxPolicy::default()),
        ("Strict", SandboxPolicy::strict()),
    ];
    for (name, policy) in &policies {
        let level = policy.evaluate(&cmd);
        println!("  {name:10} → {level}");
    }

    println!();
}

// ── Part 2 ──────────────────────────────────────────────────────────────────

async fn demo_local_sandbox() -> echo_agent::error::Result<()> {
    use echo_agent::sandbox::local::LocalConfig;

    let sandbox = LocalSandbox::new(LocalConfig {
        enable_os_sandbox: false, // 演示时不强制 OS 沙箱
        ..Default::default()
    });

    println!("  隔离级别: {}", sandbox.isolation_level());
    println!("  是否可用: {}\n", sandbox.is_available().await);

    // 基本命令
    let cmd = SandboxCommand::shell("echo 'Hello from sandbox!' && date");
    let result = sandbox.execute(cmd).await?;
    println!("  Shell 命令:");
    println!("    exit_code: {}", result.exit_code);
    println!("    stdout: {}", result.stdout.trim());
    println!("    耗时: {:?}", result.duration);
    println!("    沙箱类型: {}", result.sandbox_type);

    // 程序执行
    println!();
    let cmd = SandboxCommand::program("uname", vec!["-a".to_string()]);
    let result = sandbox.execute(cmd).await?;
    println!("  程序执行 (uname -a):");
    println!("    {}", result.stdout.trim());

    // 环境变量
    println!();
    let cmd = SandboxCommand::shell("echo \"User=$SANDBOX_USER, Mode=$SANDBOX_MODE\"")
        .with_env("SANDBOX_USER", "demo")
        .with_env("SANDBOX_MODE", "test");
    let result = sandbox.execute(cmd).await?;
    println!("  环境变量注入:");
    println!("    {}", result.stdout.trim());

    // 超时测试
    println!();
    let cmd = SandboxCommand::shell("sleep 10").with_timeout(Duration::from_millis(500));
    let result = sandbox.execute(cmd).await?;
    println!("  超时测试 (500ms timeout):");
    println!("    timed_out: {}", result.timed_out);
    println!("    耗时: {:?}", result.duration);

    // 错误处理
    println!();
    let cmd = SandboxCommand::shell("exit 1");
    let result = sandbox.execute(cmd).await?;
    println!("  失败命令 (exit 1):");
    println!(
        "    exit_code: {}, success: {}",
        result.exit_code,
        result.success()
    );

    println!();
    Ok(())
}

// ── Part 3 ──────────────────────────────────────────────────────────────────

async fn demo_docker_sandbox() -> echo_agent::error::Result<()> {
    use echo_agent::sandbox::docker::DockerConfig;

    let sandbox = DockerSandbox::new(DockerConfig::default());
    let available = sandbox.is_available().await;

    println!("  Docker 可用: {available}");
    println!("  隔离级别: {}\n", sandbox.isolation_level());

    if !available {
        println!("  ⚠️  Docker 不可用，跳过容器沙箱演示");
        println!("  💡 安装 Docker Desktop 后可体验容器级隔离\n");
        return Ok(());
    }

    // Docker 执行
    let cmd = SandboxCommand::shell("echo 'Hello from Docker!' && cat /etc/os-release | head -2");
    let result = sandbox.execute(cmd).await?;
    println!("  Docker Shell:");
    println!("    stdout: {}", result.stdout.trim());
    println!("    沙箱类型: {}", result.sandbox_type);
    println!("    耗时: {:?}", result.duration);

    // Python 代码
    println!();
    let cmd = SandboxCommand::code("python", "import sys; print(f'Python {sys.version}')");
    let result = sandbox.execute(cmd).await?;
    println!("  Docker Python:");
    println!("    {}", result.stdout.trim());

    // 资源限制
    println!();
    let cmd = SandboxCommand::shell("echo 'limited execution'");
    let limits = ResourceLimits::strict();
    let result = sandbox.execute_with_limits(cmd, limits).await?;
    println!("  资源受限执行:");
    println!(
        "    exit_code: {}, stdout: {}",
        result.exit_code,
        result.stdout.trim()
    );

    println!();
    Ok(())
}

// ── Part 4 ──────────────────────────────────────────────────────────────────

async fn demo_manager() -> echo_agent::error::Result<()> {
    let manager = SandboxManager::auto_detect().await;

    println!("  可用隔离级别: {:?}\n", manager.available_levels());

    // 安全命令 → 自动选择最轻量沙箱
    let cmd = SandboxCommand::shell("echo 'auto-routed'");
    let result = manager.execute(cmd).await?;
    println!("  安全命令 'echo':");
    println!("    → {} ({})", result.sandbox_type, result.stdout.trim());

    // 脚本命令 → 可能升级到 OS 沙箱
    let cmd = SandboxCommand::shell("python3 -c 'print(2+2)'");
    let result = manager.execute(cmd).await;
    match result {
        Ok(r) => println!(
            "  脚本命令 'python3': → {} ({})",
            r.sandbox_type,
            r.stdout.trim()
        ),
        Err(e) => println!("  脚本命令 'python3': → 执行失败: {e}"),
    }

    // 危险命令 → 优先升级到容器；若当前机器无可用 executor，则仅展示错误
    let cmd = SandboxCommand::shell("curl --version 2>/dev/null || echo 'curl not available'");
    match manager.execute(cmd).await {
        Ok(result) => {
            println!(
                "  危险命令 'curl': → {} (exit={})",
                result.sandbox_type, result.exit_code
            );
        }
        Err(e) => {
            println!("  危险命令 'curl': → 无可用沙箱接管 ({e})");
        }
    }

    println!();
    Ok(())
}

// ── Part 5 ──────────────────────────────────────────────────────────────────

async fn demo_resource_limits() -> echo_agent::error::Result<()> {
    let manager = SandboxManager::auto_detect().await;

    println!("  ResourceLimits 预设:\n");

    let default_limits = ResourceLimits::default();
    println!("  Default:");
    println!(
        "    CPU: {}s, Memory: {} MB, Output: {} KB, Processes: {}, Network: {}",
        default_limits.cpu_time_secs.unwrap_or(0),
        default_limits.memory_bytes.unwrap_or(0) / 1024 / 1024,
        default_limits.max_output_bytes.unwrap_or(0) / 1024,
        default_limits.max_processes.unwrap_or(0),
        default_limits.network,
    );

    let strict_limits = ResourceLimits::strict();
    println!("  Strict:");
    println!(
        "    CPU: {}s, Memory: {} MB, Output: {} KB, Processes: {}, Network: {}",
        strict_limits.cpu_time_secs.unwrap_or(0),
        strict_limits.memory_bytes.unwrap_or(0) / 1024 / 1024,
        strict_limits.max_output_bytes.unwrap_or(0) / 1024,
        strict_limits.max_processes.unwrap_or(0),
        strict_limits.network,
    );

    let unrestricted = ResourceLimits::unrestricted();
    println!("  Unrestricted:");
    println!(
        "    CPU: {:?}, Memory: {:?}, Network: {}\n",
        unrestricted.cpu_time_secs, unrestricted.memory_bytes, unrestricted.network,
    );

    // 超时限制演示
    let cmd = SandboxCommand::shell("for i in 1 2 3; do echo \"line $i\"; done");
    let limits = ResourceLimits {
        cpu_time_secs: Some(5),
        ..ResourceLimits::strict()
    };
    println!("  限制资源执行:");
    match manager.execute_with_limits(cmd, limits).await {
        Ok(result) => {
            println!("    exit_code: {}", result.exit_code);
            println!("    stdout: {}", result.stdout.trim());
            println!("    耗时: {:?}", result.duration);
        }
        Err(e) => {
            println!("    跳过：当前环境无可用沙箱满足资源限制 ({e})");
        }
    }

    println!();
    Ok(())
}

// ── 辅助 ────────────────────────────────────────────────────────────────────

fn print_banner() {
    println!("{}", "═".repeat(64));
    println!("      Echo Agent × 三层沙箱执行系统 (demo29)");
    println!("      Local / Docker / K8s");
    println!("{}", "═".repeat(64));
    println!();
}

fn separator(title: &str) {
    println!("{}", "─".repeat(64));
    println!("{title}\n");
}
