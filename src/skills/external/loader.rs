use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::error::{ReactError, Result};

use super::types::{LoadedSkill, ResourceRef, SkillMeta};

const SKILL_FILE: &str = "SKILL.md";

// ── SkillLoader ───────────────────────────────────────────────────────────────

/// 外部技能加载器
///
/// 负责扫描技能目录、解析 SKILL.md frontmatter，并提供资源懒加载能力。
///
/// # 目录结构约定
///
/// ```text
/// skills/
/// ├── code_review/
/// │   ├── SKILL.md          ← 必须，包含 YAML frontmatter
/// │   ├── checklist.md      ← 可选，通过 resources 引用
/// │   └── style_guide.md    ← 可选，通过 resources 引用
/// └── data_analyst/
///     ├── SKILL.md
///     └── templates/
///         └── report.md
/// ```
///
/// # SKILL.md 格式
///
/// ```markdown
/// ---
/// name: code_review
/// version: "1.0.0"
/// description: "代码审查技能"
/// tags: [code, review]
/// instructions: |
///   ## 代码审查指引
///   ...
/// resources:
///   - name: checklist
///     path: checklist.md
///     description: "审查清单"
/// ---
///
/// （frontmatter 以下的 Markdown 正文不会自动加载，仅作文档用途）
/// ```
pub struct SkillLoader {
    /// 技能根目录（每个子目录对应一个 Skill）
    skills_dir: PathBuf,

    /// 已加载的技能：skill_name → LoadedSkill
    skills: HashMap<String, LoadedSkill>,

    /// 资源内容缓存：(skill_name, resource_name) → file_content
    ///
    /// 避免重复读取磁盘，同时支持懒加载语义。
    resource_cache: HashMap<(String, String), String>,
}

impl SkillLoader {
    /// 创建 SkillLoader，指定技能根目录
    pub fn new(skills_dir: impl Into<PathBuf>) -> Self {
        Self {
            skills_dir: skills_dir.into(),
            skills: HashMap::new(),
            resource_cache: HashMap::new(),
        }
    }

    // ── 扫描与加载 ────────────────────────────────────────────────────────────

    /// 扫描技能根目录，加载所有子目录中的 SKILL.md（只读 frontmatter）
    ///
    /// 返回成功加载的 `LoadedSkill` 列表。
    /// 解析失败的技能目录会记录警告日志并跳过，不影响其他技能加载。
    pub async fn scan(&mut self) -> Result<Vec<LoadedSkill>> {
        if !self.skills_dir.exists() {
            warn!("skills 目录不存在: {}，跳过扫描", self.skills_dir.display());
            return Ok(vec![]);
        }

        let mut entries = tokio::fs::read_dir(&self.skills_dir)
            .await
            .map_err(|e| ReactError::Other(format!("无法读取 skills 目录: {}", e)))?;

        let mut loaded = Vec::new();

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| ReactError::Other(format!("遍历 skills 目录失败: {}", e)))?
        {
            let skill_dir = entry.path();

            // 只处理目录
            if !skill_dir.is_dir() {
                continue;
            }

            let skill_file = skill_dir.join(SKILL_FILE);
            if !skill_file.exists() {
                debug!("目录 '{}' 中没有 SKILL.md，跳过", skill_dir.display());
                continue;
            }

            match self.load_skill_file(&skill_dir, &skill_file).await {
                Ok(loaded_skill) => {
                    info!(
                        "已加载 Skill '{}' ({})",
                        loaded_skill.meta.name,
                        skill_dir.display()
                    );
                    loaded.push(loaded_skill.clone());
                    self.skills
                        .insert(loaded_skill.meta.name.clone(), loaded_skill);
                }
                Err(e) => {
                    warn!("加载 '{}' 失败，跳过: {}", skill_file.display(), e);
                }
            }
        }

        info!("技能扫描完成，共加载 {} 个技能", loaded.len());
        Ok(loaded)
    }

    /// 读取并解析单个 SKILL.md 文件
    async fn load_skill_file(
        &mut self,
        skill_dir: &Path,
        skill_file: &Path,
    ) -> Result<LoadedSkill> {
        let content = tokio::fs::read_to_string(skill_file)
            .await
            .map_err(|e| ReactError::Other(format!("读取 SKILL.md 失败: {}", e)))?;

        let meta = Self::parse_frontmatter(&content)?;

        // 立即加载 load_on_startup 的资源
        for res_ref in meta.startup_resources() {
            let res_path = skill_dir.join(&res_ref.path);
            match tokio::fs::read_to_string(&res_path).await {
                Ok(content) => {
                    info!(
                        "  预加载资源 '{}/{}' ({})",
                        meta.name,
                        res_ref.name,
                        res_path.display()
                    );
                    self.resource_cache
                        .insert((meta.name.clone(), res_ref.name.clone()), content);
                }
                Err(e) => {
                    warn!("  预加载资源 '{}/{}' 失败: {}", meta.name, res_ref.name, e);
                }
            }
        }

        Ok(LoadedSkill {
            meta,
            skill_dir: skill_dir.to_path_buf(),
        })
    }

    // ── Frontmatter 解析 ──────────────────────────────────────────────────────

    /// 从 Markdown 文件内容中提取并解析 YAML Frontmatter
    ///
    /// 格式：文件以 `---\n` 开头，第一个 `\n---` 之前的部分为 YAML。
    pub fn parse_frontmatter(content: &str) -> Result<SkillMeta> {
        let content = content.trim_start();

        // 必须以 --- 开头
        if !content.starts_with("---") {
            return Err(ReactError::Other(
                "SKILL.md 必须以 YAML Frontmatter（---）开头".to_string(),
            ));
        }

        // 跳过第一行的 ---
        let after_open = content
            .get(3..)
            .unwrap_or("")
            .trim_start_matches('\r')
            .trim_start_matches('\n');

        // 查找关闭的 ---（允许 \r\n 或 \n 换行）
        let close_idx = after_open
            .find("\n---")
            .ok_or_else(|| ReactError::Other("SKILL.md frontmatter 未找到结束 ---".to_string()))?;

        let yaml_str = &after_open[..close_idx];

        let meta: SkillMeta = serde_yaml::from_str(yaml_str)
            .map_err(|e| ReactError::Other(format!("SKILL.md frontmatter YAML 解析失败: {}", e)))?;

        Ok(meta)
    }

    // ── 资源懒加载 ────────────────────────────────────────────────────────────

    /// 按需加载指定技能的指定资源文件
    ///
    /// 已加载的资源直接从缓存返回，不重复读取磁盘。
    ///
    /// # 参数
    /// - `skill_name`: 技能名称（与 frontmatter.name 一致）
    /// - `resource_name`: 资源名称（与 frontmatter.resources[].name 一致）
    pub async fn load_resource(&mut self, skill_name: &str, resource_name: &str) -> Result<String> {
        let cache_key = (skill_name.to_string(), resource_name.to_string());

        // 命中缓存
        if let Some(cached) = self.resource_cache.get(&cache_key) {
            debug!("资源缓存命中: '{}/{}'", skill_name, resource_name);
            return Ok(cached.clone());
        }

        // 查找技能
        let loaded_skill = self
            .skills
            .get(skill_name)
            .ok_or_else(|| ReactError::Other(format!("未知技能: '{}'", skill_name)))?;

        // 查找资源定义
        let resource_ref = loaded_skill
            .meta
            .resources
            .as_ref()
            .and_then(|rs| rs.iter().find(|r| r.name == resource_name))
            .ok_or_else(|| {
                ReactError::Other(format!(
                    "技能 '{}' 中没有名为 '{}' 的资源",
                    skill_name, resource_name
                ))
            })?
            .clone();

        let resource_path = loaded_skill.skill_dir.join(&resource_ref.path);

        if !resource_path.exists() {
            return Err(ReactError::Other(format!(
                "资源文件不存在: {}",
                resource_path.display()
            )));
        }

        let content = tokio::fs::read_to_string(&resource_path)
            .await
            .map_err(|e| {
                ReactError::Other(format!(
                    "读取资源 '{}/{}' 失败: {}",
                    skill_name, resource_name, e
                ))
            })?;

        info!(
            "已加载资源 '{}/{}' ({} 字节)",
            skill_name,
            resource_name,
            content.len()
        );

        // 写入缓存
        self.resource_cache.insert(cache_key, content.clone());
        Ok(content)
    }

    // ── 查询 API ──────────────────────────────────────────────────────────────

    /// 获取指定技能的元数据
    pub fn get_skill(&self, name: &str) -> Option<&LoadedSkill> {
        self.skills.get(name)
    }

    /// 列出所有已加载的技能
    pub fn list_skills(&self) -> Vec<&LoadedSkill> {
        let mut skills: Vec<&LoadedSkill> = self.skills.values().collect();
        skills.sort_by_key(|s| &s.meta.name);
        skills
    }

    /// 返回所有资源的目录（skill_name, resource_ref），供工具描述使用
    pub fn resource_catalog(&self) -> Vec<(String, ResourceRef)> {
        let mut catalog = Vec::new();
        for skill in self.skills.values() {
            if let Some(resources) = &skill.meta.resources {
                for res in resources {
                    catalog.push((skill.meta.name.clone(), res.clone()));
                }
            }
        }
        catalog.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
        catalog
    }

    /// 检查资源是否已缓存
    pub fn is_cached(&self, skill_name: &str, resource_name: &str) -> bool {
        self.resource_cache
            .contains_key(&(skill_name.to_string(), resource_name.to_string()))
    }

    /// 已加载的技能数量
    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }
}
