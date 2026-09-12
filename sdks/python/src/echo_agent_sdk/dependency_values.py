from enum import Enum


class DependencyKind(str, Enum):
    BINARY = "binary"
    PYTHON_PKG = "python_pkg"
    NODE_MODULE = "node_module"

    def as_str(self) -> str:
        return self.value


class SkillSource(str, Enum):
    LOCAL = "local"
    MCP = "mcp"

    def as_str(self) -> str:
        return self.value
