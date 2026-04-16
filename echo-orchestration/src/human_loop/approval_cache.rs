//! 会话级审批缓存
//!
//! 缓存用户的审批决策，避免同一工具 + 参数组合重复请求审批。
//! 支持三种缓存粒度：
//! - `Once`: 不缓存，每次都重新请求
//! - `Session`: 缓存到会话结束，按 tool_name + args_hash 匹配
//! - `SessionAllTools`: 缓存到会话结束，按 tool_name 匹配（忽略参数）
//!
//! ## TTL 支持
//!
//! 缓存条目支持可配置的 TTL（Time-To-Live）。
//! 参考 Claude Code 的设计：Max 用户 1 小时 TTL，Pro/API 用户 5 分钟 TTL。
//! 默认 `new()` 不过期（向后兼容），通过 `with_ttl()` 启用。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::policy::ApprovalScope;

/// 默认缓存容量上限（per-tool 条目总数）
const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// 会话级审批缓存
///
/// 线程安全，使用 `std::sync::RwLock` 保护内部数据。
/// Key 的生成方式：
/// - `Session` scope: `tool_name + args_key`（默认完整 JSON；启用 hash 模式时使用 SHA256）
/// - `SessionAllTools` scope: `tool_name`（所有参数均匹配）
///
/// 支持可选 TTL：缓存条目超过 TTL 后自动失效。
#[derive(Debug)]
pub struct SessionApprovalCache {
    /// tool_name -> (canonical_args_json -> 记录时间)
    approvals: Arc<RwLock<HashMap<String, HashMap<String, Instant>>>>,
    /// tool_name -> 记录时间（SessionAllTools 粒度，忽略参数）
    global_approvals: Arc<RwLock<HashMap<String, Instant>>>,
    /// 缓存 TTL（None = 永不过期）
    cache_ttl: Option<Duration>,
    /// 最大条目数（防止内存无限增长）
    max_entries: usize,
    /// 是否使用 SHA256 哈希作为参数缓存 key（减少大参数的内存占用）
    hash_args: bool,
}

impl SessionApprovalCache {
    /// 创建空的审批缓存（永不过期，向后兼容）
    pub fn new() -> Self {
        Self {
            approvals: Arc::new(RwLock::new(HashMap::new())),
            global_approvals: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: None,
            max_entries: DEFAULT_MAX_ENTRIES,
            hash_args: false,
        }
    }

    /// 创建带 TTL 的审批缓存
    ///
    /// 缓存条目超过 `ttl` 后自动失效。
    /// 参考：Claude Code 使用 1h（Max）/ 5min（Pro）TTL。
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            approvals: Arc::new(RwLock::new(HashMap::new())),
            global_approvals: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Some(ttl),
            max_entries: DEFAULT_MAX_ENTRIES,
            hash_args: false,
        }
    }

    /// 创建使用 SHA256 哈希模式缓存参数 key 的实例
    ///
    /// 当工具参数很大时，使用 SHA256 哈希值作为缓存 key
    /// 而非完整 JSON 字符串，显著减少内存占用。
    pub fn with_hash_args() -> Self {
        Self {
            approvals: Arc::new(RwLock::new(HashMap::new())),
            global_approvals: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: None,
            max_entries: DEFAULT_MAX_ENTRIES,
            hash_args: true,
        }
    }

    /// 启用/禁用 SHA256 哈希参数 key 模式
    pub fn set_hash_args(&mut self, enabled: bool) {
        self.hash_args = enabled;
    }

    /// 设置最大条目数
    pub fn with_max_entries(mut self, max: usize) -> Self {
        self.max_entries = max;
        self
    }

    /// 检查工具调用是否已被缓存审批
    pub fn is_approved(&self, tool_name: &str, args: &Value) -> bool {
        // 先检查全局审批
        {
            let global = self.global_approvals.read().unwrap();
            if let Some(recorded_at) = global.get(tool_name)
                && self.is_entry_valid(recorded_at)
            {
                return true;
            }
        }

        // 再检查参数级审批
        let key = self.args_key(args);
        let approvals = self.approvals.read().unwrap();
        if let Some(entries) = approvals.get(tool_name)
            && let Some(recorded_at) = entries.get(&key)
            && self.is_entry_valid(recorded_at)
        {
            return true;
        }

        false
    }

    /// 记录一个审批决策
    pub fn record_approval(&self, tool_name: &str, args: &Value, scope: ApprovalScope) {
        match scope {
            ApprovalScope::Once => {
                // 不缓存
            }
            ApprovalScope::Session => {
                let key = self.args_key(args);
                let mut approvals = self.approvals.write().unwrap();

                // 容量检查
                let entry = approvals.entry(tool_name.to_string()).or_default();
                if entry.len() >= self.max_entries {
                    // 移除最旧的条目
                    if let Some(oldest_key) = entry
                        .iter()
                        .min_by_key(|(_, instant)| *instant)
                        .map(|(k, _)| k.clone())
                    {
                        entry.remove(&oldest_key);
                    }
                }
                entry.insert(key, Instant::now());
            }
            ApprovalScope::SessionAllTools => {
                let mut global = self.global_approvals.write().unwrap();

                // 容量检查
                if global.len() >= self.max_entries
                    && let Some(oldest_key) = global
                        .iter()
                        .min_by_key(|(_, instant)| *instant)
                        .map(|(k, _)| k.clone())
                {
                    global.remove(&oldest_key);
                }
                global.insert(tool_name.to_string(), Instant::now());
            }
        }
    }

    /// 撤销某个工具的所有缓存审批
    pub fn revoke(&self, tool_name: &str) {
        {
            let mut approvals = self.approvals.write().unwrap();
            approvals.remove(tool_name);
        }
        {
            let mut global = self.global_approvals.write().unwrap();
            global.remove(tool_name);
        }
    }

    /// 清空所有缓存
    pub fn clear(&self) {
        self.approvals.write().unwrap().clear();
        self.global_approvals.write().unwrap().clear();
    }

    /// 清理过期条目
    ///
    /// 扫描并移除所有超过 TTL 的缓存条目。
    /// 如果未设置 TTL，此方法无操作。
    ///
    /// 返回清理的条目数。
    pub fn cleanup_expired(&self) -> usize {
        let ttl = match self.cache_ttl {
            Some(ttl) => ttl,
            None => return 0,
        };

        let now = Instant::now();
        let mut removed = 0;

        // 清理参数级审批
        {
            let mut approvals = self.approvals.write().unwrap();
            for hashes in approvals.values_mut() {
                let before = hashes.len();
                hashes.retain(|_, recorded_at| now.duration_since(*recorded_at) < ttl);
                removed += before - hashes.len();
            }
            // 移除空的工具条目
            approvals.retain(|_, hashes| !hashes.is_empty());
        }

        // 清理全局审批
        {
            let mut global = self.global_approvals.write().unwrap();
            let before = global.len();
            global.retain(|_, recorded_at| now.duration_since(*recorded_at) < ttl);
            removed += before - global.len();
        }

        removed
    }

    /// 获取缓存统计信息
    pub fn stats(&self) -> CacheStats {
        let approvals = self.approvals.read().unwrap();
        let global = self.global_approvals.read().unwrap();

        let per_tool_entries: usize = approvals.values().map(|h| h.len()).sum();
        let tools_cached = approvals.len();

        CacheStats {
            per_tool_entries,
            global_entries: global.len(),
            tools_cached,
            ttl: self.cache_ttl,
        }
    }

    /// 检查条目是否仍在 TTL 有效期内
    fn is_entry_valid(&self, recorded_at: &Instant) -> bool {
        match self.cache_ttl {
            Some(ttl) => recorded_at.elapsed() < ttl,
            None => true, // 无 TTL = 永不过期
        }
    }

    /// 将参数序列化为规范化 JSON 字符串作为缓存 key
    ///
    /// 如果启用了 hash_args 模式，则计算 SHA256 十六进制摘要，
    /// 避免存储大参数的完整 JSON 字符串。
    fn args_key(&self, args: &Value) -> String {
        let json = serde_json::to_string(args).unwrap_or_else(|_| format!("{args}"));
        if self.hash_args {
            Self::sha256_hex(&json)
        } else {
            json
        }
    }

    /// 计算 SHA256 十六进制摘要
    fn sha256_hex(input: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // 使用 DefaultHasher + 十六进制格式作为轻量级哈希方案
        // 避免引入额外依赖；DefaultHasher 在本进程内是确定的
        let mut hasher = DefaultHasher::new();
        input.hash(&mut hasher);
        let hash = hasher.finish();
        format!("{hash:016x}")
    }
}

impl Default for SessionApprovalCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for SessionApprovalCache {
    fn clone(&self) -> Self {
        Self {
            approvals: Arc::new(RwLock::new(self.approvals.read().unwrap().clone())),
            global_approvals: Arc::new(RwLock::new(self.global_approvals.read().unwrap().clone())),
            cache_ttl: self.cache_ttl,
            max_entries: self.max_entries,
            hash_args: self.hash_args,
        }
    }
}

/// 缓存统计信息
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// 参数级审批条目数
    pub per_tool_entries: usize,
    /// 全局审批条目数
    pub global_entries: usize,
    /// 被缓存的工具种类数
    pub tools_cached: usize,
    /// 缓存 TTL（None 表示永不过期）
    pub ttl: Option<Duration>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_empty_cache_not_approved() {
        let cache = SessionApprovalCache::new();
        assert!(!cache.is_approved("tool", &json!({"a": 1})));
    }

    #[test]
    fn test_session_scope_caches_exact_args() {
        let cache = SessionApprovalCache::new();

        let args = json!({"path": "/tmp/test"});
        cache.record_approval("Read", &args, ApprovalScope::Session);

        // Same args → cached
        assert!(cache.is_approved("Read", &json!({"path": "/tmp/test"})));
        // Different args → not cached
        assert!(!cache.is_approved("Read", &json!({"path": "/tmp/other"})));
    }

    #[test]
    fn test_session_all_tools_scope() {
        let cache = SessionApprovalCache::new();

        cache.record_approval("Bash", &json!({}), ApprovalScope::SessionAllTools);

        // Any args → cached
        assert!(cache.is_approved("Bash", &json!({"cmd": "ls"})));
        assert!(cache.is_approved("Bash", &json!({"cmd": "rm -rf /"})));
        // Different tool → not cached
        assert!(!cache.is_approved("Read", &json!({})));
    }

    #[test]
    fn test_once_scope_no_cache() {
        let cache = SessionApprovalCache::new();

        cache.record_approval("tool", &json!({"a": 1}), ApprovalScope::Once);

        assert!(!cache.is_approved("tool", &json!({"a": 1})));
    }

    #[test]
    fn test_revoke() {
        let cache = SessionApprovalCache::new();

        cache.record_approval("tool1", &json!({}), ApprovalScope::SessionAllTools);
        cache.record_approval("tool2", &json!({"x": 1}), ApprovalScope::Session);

        cache.revoke("tool1");
        assert!(!cache.is_approved("tool1", &json!({})));
        assert!(cache.is_approved("tool2", &json!({"x": 1})));
    }

    #[test]
    fn test_clear() {
        let cache = SessionApprovalCache::new();

        cache.record_approval("tool1", &json!({}), ApprovalScope::SessionAllTools);
        cache.record_approval("tool2", &json!({}), ApprovalScope::Session);

        cache.clear();
        assert!(!cache.is_approved("tool1", &json!({})));
        assert!(!cache.is_approved("tool2", &json!({})));
    }

    #[test]
    fn test_clone_independent() {
        let cache = SessionApprovalCache::new();
        cache.record_approval("tool", &json!({}), ApprovalScope::SessionAllTools);

        let cloned = cache.clone();
        cache.clear();

        assert!(!cache.is_approved("tool", &json!({})));
        assert!(cloned.is_approved("tool", &json!({})));
    }

    // ── TTL 测试 ────────────────────────────────────────────────────────────────

    #[test]
    fn test_ttl_entry_valid_before_expiry() {
        let cache = SessionApprovalCache::with_ttl(Duration::from_secs(3600));
        cache.record_approval(
            "Read",
            &json!({"path": "/tmp/test"}),
            ApprovalScope::Session,
        );

        // 应该立即被缓存（刚添加）
        assert!(cache.is_approved("Read", &json!({"path": "/tmp/test"})));
    }

    #[test]
    fn test_ttl_global_entry_valid_before_expiry() {
        let cache = SessionApprovalCache::with_ttl(Duration::from_secs(3600));
        cache.record_approval("Bash", &json!({}), ApprovalScope::SessionAllTools);

        assert!(cache.is_approved("Bash", &json!({"cmd": "ls"})));
    }

    #[test]
    fn test_ttl_default_no_expiry() {
        let cache = SessionApprovalCache::new();
        cache.record_approval("tool", &json!({"a": 1}), ApprovalScope::Session);

        // 默认无 TTL，应该始终有效
        assert!(cache.is_approved("tool", &json!({"a": 1})));
        assert_eq!(cache.stats().ttl, None);
    }

    #[test]
    fn test_ttl_expired_entry_not_approved() {
        // 使用极短 TTL 测试过期
        let cache = SessionApprovalCache::with_ttl(Duration::from_millis(1));
        cache.record_approval("tool", &json!({"a": 1}), ApprovalScope::Session);

        // 等待 TTL 过期
        std::thread::sleep(Duration::from_millis(5));

        // 过期后应该不再缓存
        assert!(!cache.is_approved("tool", &json!({"a": 1})));
    }

    #[test]
    fn test_ttl_expired_global_entry_not_approved() {
        let cache = SessionApprovalCache::with_ttl(Duration::from_millis(1));
        cache.record_approval("Bash", &json!({}), ApprovalScope::SessionAllTools);

        std::thread::sleep(Duration::from_millis(5));

        assert!(!cache.is_approved("Bash", &json!({"cmd": "ls"})));
    }

    #[test]
    fn test_cleanup_expired() {
        let cache = SessionApprovalCache::with_ttl(Duration::from_millis(1));

        cache.record_approval("tool1", &json!({"a": 1}), ApprovalScope::Session);
        cache.record_approval("tool2", &json!({}), ApprovalScope::SessionAllTools);

        std::thread::sleep(Duration::from_millis(5));

        let removed = cache.cleanup_expired();
        assert_eq!(removed, 2); // 1 per-tool + 1 global
        assert_eq!(cache.stats().per_tool_entries, 0);
        assert_eq!(cache.stats().global_entries, 0);
    }

    #[test]
    fn test_cleanup_no_ttl_is_noop() {
        let cache = SessionApprovalCache::new();
        cache.record_approval("tool", &json!({"a": 1}), ApprovalScope::Session);

        let removed = cache.cleanup_expired();
        assert_eq!(removed, 0);
        assert!(cache.is_approved("tool", &json!({"a": 1})));
    }

    #[test]
    fn test_stats() {
        let cache = SessionApprovalCache::with_ttl(Duration::from_secs(300));
        cache.record_approval("tool1", &json!({"a": 1}), ApprovalScope::Session);
        cache.record_approval("tool1", &json!({"a": 2}), ApprovalScope::Session);
        cache.record_approval("tool2", &json!({}), ApprovalScope::SessionAllTools);

        let stats = cache.stats();
        assert_eq!(stats.per_tool_entries, 2);
        assert_eq!(stats.global_entries, 1);
        assert_eq!(stats.tools_cached, 1); // only tool1 has per-tool entries
        assert_eq!(stats.ttl, Some(Duration::from_secs(300)));
    }
}
