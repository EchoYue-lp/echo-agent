//! 内容搜索的可选远端接缝。
//!
//! 现有 `grep` 实现是本机遍历：枚举目录、逐文件 `FileSystem::read` 取正文、
//! 再在本地做正则匹配。远程工作区（例如 SSH 项目）因此必须把整棵候选树的
//! 正文全部拉回本机才能回答一次搜索。本接缝让提供者在远端完成匹配、只回传
//! 已经排版好的结果记录；不支持的提供者返回 `None`，调用方落回原本机遍历。
//!
//! 这里只定义传输形状，不定义搜索算法：正则语义、忽略目录、大小上限等
//! 仍由原工具持有，提供者必须复现同一语义，否则远端与回退路径的结果不可比。

use std::path::Path;

use crate::error::Result;

/// 一次内容搜索请求，字段语义与 `grep` 工具入参一致。
///
/// `root` 是工具已经解析好的实际搜索目标（文件或目录），不是用户原始字符串；
/// `pattern` 是正则源码而不是编译产物，提供者按自己的传输方式匹配。
#[derive(Debug, Clone, Copy)]
pub struct ContentSearchRequest<'a> {
    pub root: &'a Path,
    pub pattern: &'a str,
    pub glob: Option<&'a str>,
    pub context_lines: usize,
    /// 必须与本机 `RegexBuilder::case_insensitive` 一致，否则同一请求在
    /// 远端与回退路径会得到不同命中集合。
    pub case_insensitive: bool,
    /// 单次请求允许回传的记录条数上界，不是结果页大小。
    ///
    /// 它刻意与 `PageRequest::limit` 无关：分页指纹覆盖结果集合本身，若上界
    /// 随页大小变化，同一查询的后续页会因快照不同而失配。提供者可以把结果
    /// 截断到该上界，但必须在 [`ContentSearchOutcome::truncated`] 中如实声明。
    pub max_results: usize,
}

/// 提供者已完成排版的结果记录。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentSearchOutcome {
    /// 逐条对应结果页的一项，格式必须与工具最终展示格式一致：
    /// 无上下文行为 `"{path}:{line}:  {content}"`；有上下文行时为多行记录，
    /// 命中行前缀 `>`、上下文行前缀空格，每行形如
    /// `"{prefix}{path}:{line}:  {content}"`。
    ///
    /// 调用方只做分页与拼接，不再重新排版，因此这里给的是最终展示文本。
    pub records: Vec<String>,
    /// 记录只是完整命中集的前缀（因 `max_results` 截断），调用方需要把
    /// 结果标记为不完整，不能让它看起来是全部命中。
    pub truncated: bool,
}

/// 在远端完成内容匹配的可选提供者。
///
/// 契约：
/// - `Ok(None)` 表示“本提供者不支持这类请求”（例如本机文件系统提供者），
///   调用方必须落回本机遍历，且回退结果与未接入本接缝时逐字相同。
/// - `Err` 表示提供者支持该请求但本次搜索失败。调用方按工具既有错误映射
///   返回失败，不得静默降级——否则一次远端故障会被呈现成“没有匹配”。
/// - 实现必须是只读的：本接缝只用于搜索，不改变目标内容。
pub trait ContentSearchProvider: Send + Sync {
    fn search(&self, request: &ContentSearchRequest<'_>) -> Result<Option<ContentSearchOutcome>>;
}
