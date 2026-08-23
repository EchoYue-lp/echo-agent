use echo_agent::config::FrameworkConfig;
use echo_agent::paths::DataRoot;
use echo_agent::tools::{StandardToolPack, ToolPack};

#[test]
fn public_facade_composes_without_split_crates() {
    let config = FrameworkConfig::default();
    let root = DataRoot::new("/tmp/echo-agent-smoke");
    let pack = StandardToolPack::new();

    assert!(config.model.name.is_empty());
    assert_eq!(
        root.path("state.json"),
        std::path::PathBuf::from("/tmp/echo-agent-smoke/state.json")
    );
    assert_eq!(pack.name(), "standard");
}
