//! 应用数据目录解析（框架层可配置的单一事实源）。
//!
//! 框架把用户级数据(config、记忆 store、trajectories、curator 状态等)统一放在
//! 用户 home 下的**一个基础目录**里。这个目录的**名字是应用/产品决策,不是框架
//! 决策**(见 AGENTS.md「框架 vs 应用」):通用框架默认用 `~/.echo-agent`,而基于
//! 它构建的产品(如 EKO)在启动时调用 [`set_user_data_dir_name`] 把它改成自己的
//! 品牌目录(`~/.eko`)。
//!
//! 这样框架保持中性可复用,又让上层应用统一切换根目录——而不用在每个 callsite
//! 手写 `~/.echo-agent` 前缀。
//!
//! # 用法(应用侧)
//!
//! ```no_run
//! // 在 main() 最早期、任何路径解析之前调用一次:
//! let _ = echo_agent::paths::set_user_data_dir_name(".eko");
//! // 之后框架与应用都通过 user_data_dir() 取同一个根:
//! let store = echo_agent::paths::user_data_path("store.json");
//! ```

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 框架默认的用户数据目录名(位于用户 home 下)。
pub const DEFAULT_USER_DATA_DIR_NAME: &str = ".echo-agent";

static USER_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 解析用户 home 目录。
///
/// 与框架既有代码一致,优先读 `HOME` 环境变量;不可用时回退到当前目录 `.`,
/// 保证永不 panic(AGENTS.md 硬性约束)。
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 用一个显式的绝对路径覆盖基础用户数据目录。
///
/// 必须在应用启动**最早期**、任何 [`user_data_dir`] 解析之前调用一次。若目录已被
/// 初始化:值相同视为幂等成功返回 `Ok(())`;值不同返回 `Err(当前已生效的值)`,
/// 以便调用方发现「设置得太晚」的问题。
pub fn set_user_data_dir(dir: impl Into<PathBuf>) -> Result<(), PathBuf> {
    let dir = dir.into();
    match USER_DATA_DIR.set(dir.clone()) {
        Ok(()) => Ok(()),
        Err(_) => {
            let current = user_data_dir();
            if current == dir { Ok(()) } else { Err(current) }
        }
    }
}

/// 便捷方法:把基础用户数据目录设为 `~/<name>`(例如 `.eko`)。
///
/// 这是应用切换品牌根目录的推荐入口。语义同 [`set_user_data_dir`]。
pub fn set_user_data_dir_name(name: impl AsRef<str>) -> Result<(), PathBuf> {
    set_user_data_dir(home_dir().join(name.as_ref()))
}

/// 解析基础用户数据目录。未被覆盖时默认 `~/.echo-agent`。
///
/// 首次调用会把默认值锁定进 `OnceLock`;因此应用要覆盖必须在此之前调用 setter。
pub fn user_data_dir() -> PathBuf {
    USER_DATA_DIR
        .get_or_init(|| home_dir().join(DEFAULT_USER_DATA_DIR_NAME))
        .clone()
}

/// 在基础用户数据目录下拼接子路径。
pub fn user_data_path(child: impl AsRef<Path>) -> PathBuf {
    user_data_dir().join(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_path_joins_under_base() {
        // 不调用 setter,验证默认根语义与 join 行为(不依赖全局是否已被其它测试初始化)。
        let base = user_data_dir();
        assert_eq!(user_data_path("store.json"), base.join("store.json"));
        assert!(base.ends_with(DEFAULT_USER_DATA_DIR_NAME) || base.ends_with(".eko"));
    }

    #[test]
    fn set_same_value_is_idempotent() {
        let current = user_data_dir();
        // 用当前已生效值再 set 一次应为 Ok(幂等)。
        assert!(set_user_data_dir(current).is_ok());
    }
}
