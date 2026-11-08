//! 测试专用持久化/诊断根，不参与生产路径或用户数据根选择。

use crate::error::{ReactError, Result};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// 显式覆盖优先；普通 cargo test 在当前 checkout 内拥有安全默认根。
/// 父测试须把返回值通过 Command::env 传给子进程，避免修改并行测试的全局环境。
pub(super) fn root() -> Result<PathBuf> {
    root_for(
        std::env::var_os("EKO_CONSUMER_TEST_ROOT"),
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

fn root_for(override_root: Option<OsString>, manifest_dir: &Path) -> Result<PathBuf> {
    let repository = std::fs::canonicalize(manifest_dir)?;
    // 主工作区中的独立子仓可使用 superproject 的 ignored 工程目录；
    // CI 独立 checkout 的父目录没有 Git 根标记，默认仍局限于本仓。
    let checkout = repository
        .parent()
        .filter(|parent| parent.join(".git").exists())
        .unwrap_or(&repository);
    let checkout = std::fs::canonicalize(checkout)?;
    let root = override_root.map(PathBuf::from).unwrap_or_else(|| {
        repository
            .join(".echo-semantic")
            .join("local")
            .join("consumer-contract-tests")
    });
    // 本地也需要此约束：测试配置拼错或符号链接不能把合成历史/诊断写入
    // 用户目录、源码 checkout 之外；不对生产 Agent 增加权限门控。
    if !root.is_absolute() || root.components().any(|part| part == Component::ParentDir) {
        return Err(ReactError::Other(
            "EKO_CONSUMER_TEST_ROOT must be a nonempty absolute artifact directory inside the checkout"
                .to_string(),
        ));
    }
    let ancestor = root.ancestors().find(|path| path.exists()).ok_or_else(|| {
        ReactError::Other("consumer test root has no existing parent".to_string())
    })?;
    let canonical_ancestor = std::fs::canonicalize(ancestor)?;
    if !canonical_ancestor.starts_with(&checkout) {
        return Err(ReactError::Other(
            "EKO_CONSUMER_TEST_ROOT resolves outside the checkout".to_string(),
        ));
    }
    // 先解析已存在的父目录，再补尚未创建的相对尾部；这样既保留合法的
    // checkout 路径别名，也兼容 Windows canonicalize 的 verbatim 前缀。
    let tail = root.strip_prefix(ancestor).map_err(|error| {
        ReactError::Other(format!("invalid consumer test root parent: {error}"))
    })?;
    let root = canonical_ancestor.join(tail);
    if root == checkout {
        return Err(ReactError::Other(
            "EKO_CONSUMER_TEST_ROOT must name an artifact subdirectory".to_string(),
        ));
    }
    std::fs::create_dir_all(&root)?;
    let root = std::fs::canonicalize(root)?;
    if !root.starts_with(&checkout) {
        return Err(ReactError::Other(
            "EKO_CONSUMER_TEST_ROOT changed outside the checkout".to_string(),
        ));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<tempfile::TempDir> {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".echo-semantic/local/consumer-root-helper-tests");
        std::fs::create_dir_all(&base)?;
        Ok(tempfile::tempdir_in(base)?)
    }

    #[test]
    fn no_override_uses_the_repository_ignored_directory() -> Result<()> {
        let fixture = fixture()?;
        let repository = fixture.path().join("repository");
        std::fs::create_dir_all(&repository)?;
        let actual = root_for(None, &repository)?;
        assert_eq!(
            actual,
            std::fs::canonicalize(repository.join(".echo-semantic/local/consumer-contract-tests"))?
        );
        Ok(())
    }

    #[test]
    fn explicit_root_in_the_superproject_is_preserved() -> Result<()> {
        let fixture = fixture()?;
        let checkout = fixture.path().join("workspace");
        let repository = checkout.join("framework");
        std::fs::create_dir_all(checkout.join(".git"))?;
        std::fs::create_dir_all(&repository)?;
        let requested = checkout.join(".echo-semantic/local/explicit-consumer");
        let actual = root_for(Some(requested.clone().into_os_string()), &repository)?;
        assert_eq!(actual, std::fs::canonicalize(requested)?);
        Ok(())
    }

    #[test]
    fn invalid_override_does_not_fall_back_or_create_an_external_directory() -> Result<()> {
        let fixture = fixture()?;
        let repository = fixture.path().join("repository");
        std::fs::create_dir_all(&repository)?;
        let outside = fixture.path().join("outside");
        for requested in [
            PathBuf::new(),
            PathBuf::from("relative"),
            repository.join("..").join("escaped"),
            outside.clone(),
        ] {
            assert!(root_for(Some(requested.into_os_string()), &repository).is_err());
        }
        assert!(!outside.exists());
        assert!(
            !repository
                .join(".echo-semantic/local/consumer-contract-tests")
                .exists()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cannot_redirect_synthetic_artifacts_outside_the_checkout() -> Result<()> {
        let fixture = fixture()?;
        let repository = fixture.path().join("repository");
        let outside = fixture.path().join("outside");
        std::fs::create_dir_all(&repository)?;
        std::fs::create_dir_all(&outside)?;
        let link = repository.join("artifact-link");
        std::os::unix::fs::symlink(&outside, &link)?;
        assert!(root_for(Some(link.join("consumer").into_os_string()), &repository).is_err());
        assert!(!outside.join("consumer").exists());
        Ok(())
    }
}
