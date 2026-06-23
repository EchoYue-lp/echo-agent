//! Lightweight dependency probe for skill scripts.
//!
//! Reads `metadata.requires-binaries` and `metadata.requires-python-packages`
//! from a [`SkillDescriptor`]'s frontmatter and produces a structured
//! [`ProbeReport`].  System binaries are probed via `which`; Python packages
//! are documentation-only (uv + PEP 723 handles them at runtime).
//!
//! Never auto-installs anything — only detects and suggests.

use crate::skills::external::types::SkillDescriptor;

/// Kind of an external dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepKind {
    /// A system binary (e.g. `soffice`, `pdftoppm`).
    Binary,
    /// A Python package (handled by uv + PEP 723; documentation-only).
    PythonPkg,
    /// A Node.js module.
    NodeModule,
}

/// A single dependency declared by a skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDependency {
    pub kind: DepKind,
    pub name: String,
    /// `true` = skill unusable without it; `false` = optional feature degrades.
    pub required: bool,
    /// Human-readable install hint (e.g. "brew install --cask libreoffice").
    pub install_hint: String,
}

/// Result of probing all declared dependencies for a skill.
#[derive(Debug, Clone, Default)]
pub struct ProbeReport {
    /// Dependencies found on the system.
    pub satisfied: Vec<String>,
    /// Required dependencies that are missing (skill cannot function).
    pub missing_required: Vec<SkillDependency>,
    /// Optional dependencies that are missing (degraded functionality).
    pub missing_optional: Vec<SkillDependency>,
}

impl ProbeReport {
    /// `true` when all required dependencies are present.
    pub fn is_ok(&self) -> bool {
        self.missing_required.is_empty()
    }
}

/// Parse a comma-separated or JSON-array-like string value from metadata
/// into a list of individual items.  Frontmatter `metadata:` values are
/// stored as `HashMap<String, String>`, so YAML lists like
/// `[soffice, pdftoppm]` come through as `"[soffice, pdftoppm]"`.
fn parse_metadata_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| {
            s.trim()
                .trim_matches(|c: char| c == '[' || c == ']' || c == '"' || c == '\'')
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Extract declared dependencies from a [`SkillDescriptor`]'s frontmatter
/// metadata.  Reads `requires-binaries` and `requires-python-packages` keys
/// from the metadata map (both stored as comma-separated strings).
pub fn extract_dependencies(desc: &SkillDescriptor) -> Vec<SkillDependency> {
    let mut deps = Vec::new();

    if let Some(bins_raw) = desc.metadata.get("requires-binaries") {
        for name in parse_metadata_list(bins_raw) {
            deps.push(SkillDependency {
                kind: DepKind::Binary,
                install_hint: install_hint_for_binary(&name),
                name,
                required: true,
            });
        }
    }

    if let Some(pkgs_raw) = desc.metadata.get("requires-python-packages") {
        for name in parse_metadata_list(pkgs_raw) {
            deps.push(SkillDependency {
                kind: DepKind::PythonPkg,
                install_hint: format!("uv run 自动处理: {}", name),
                name,
                required: false, // uv + PEP 723 handles it
            });
        }
    }

    deps
}

/// 探测单个二进制是否在 PATH 上(走 `which` 子进程,不引入 which crate)。
/// 失败(无 which 命令等)返回 false —— 偏保守,把声明二进制当缺失显示,
/// 这样用户至少能看到提示而非静默通过。
fn binary_available(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 探测一个 skill 声明的全部必需二进制,返回**缺失**的二进制名列表。
/// 供 SkillsHub scan 调用,结果存进 SkillHubEntry.missing_dependencies,
/// 前端据此显示 ⚠️ 提示。Python 包不探测(uv + PEP 723 运行时处理)。
pub fn missing_binary_names(desc: &SkillDescriptor) -> Vec<String> {
    extract_dependencies(desc)
        .into_iter()
        .filter(|d| d.required && matches!(d.kind, DepKind::Binary) && !binary_available(&d.name))
        .map(|d| d.name)
        .collect()
}

/// Return a human-readable install hint for common binaries.
fn install_hint_for_binary(bin: &str) -> String {
    match bin {
        "soffice" => "brew install --cask libreoffice".into(),
        "pdftoppm" => "brew install poppler".into(),
        "ffmpeg" => "brew install ffmpeg".into(),
        "sqlite3" => "pre-installed on macOS".into(),
        "git" => "pre-installed on macOS".into(),
        _ => format!("请自行安装: {}", bin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::external::types::SkillSandboxPolicy;
    use std::collections::HashMap;

    fn desc_with_metadata(md: HashMap<String, String>) -> SkillDescriptor {
        SkillDescriptor {
            name: "test-skill".into(),
            description: "test".into(),
            license: None,
            compatibility: None,
            metadata: md,
            allowed_tools: vec![],
            shell: None,
            paths: vec![],
            triggers: vec![],
            hooks: None,
            sandbox: Some(SkillSandboxPolicy::default()),
            depends_on: vec![],
            location: std::path::PathBuf::from("/tmp/test-skill"),
        }
    }

    #[test]
    fn extract_empty_for_no_metadata() {
        let desc = desc_with_metadata(HashMap::new());
        let deps = extract_dependencies(&desc);
        assert!(deps.is_empty());
    }

    #[test]
    fn extract_binaries_from_frontmatter() {
        let mut md = HashMap::new();
        md.insert("requires-binaries".into(), "soffice, pdftoppm".into());
        let desc = desc_with_metadata(md);
        let deps = extract_dependencies(&desc);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].name, "soffice");
        assert_eq!(deps[0].kind, DepKind::Binary);
        assert!(deps[0].required);
        assert_eq!(deps[1].name, "pdftoppm");
    }

    #[test]
    fn extract_python_packages_as_optional() {
        let mut md = HashMap::new();
        md.insert("requires-python-packages".into(), "defusedxml".into());
        let desc = desc_with_metadata(md);
        let deps = extract_dependencies(&desc);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "defusedxml");
        assert_eq!(deps[0].kind, DepKind::PythonPkg);
        assert!(
            !deps[0].required,
            "python packages are handled by uv + PEP 723; documentation-only"
        );
    }

    #[test]
    fn probe_report_default_is_ok() {
        let report = ProbeReport::default();
        assert!(report.is_ok());
    }
}
