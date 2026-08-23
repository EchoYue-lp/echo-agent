use echo_agent::config::FrameworkConfig;
use echo_agent::paths::DataRoot;
use echo_agent::tools::{StandardToolPack, ToolPack};

fn main() {
    let root = DataRoot::new("./agent-data");
    let config = FrameworkConfig::default();
    let pack = StandardToolPack::new();

    println!(
        "facade example: root={}, model={}, tool_pack={}",
        root.as_path().display(),
        config.model.name,
        pack.name()
    );
}
