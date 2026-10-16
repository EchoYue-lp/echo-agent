"""Generate a conservative registry-origin view for the official cargo-audit scanner."""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import re
import sys
if sys.version_info < (3, 11):
    raise SystemExit("registry-origin audit tooling requires Python 3.11 or newer")
import tomllib

REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"

def registry_origin_view(text: str, origins: list[dict]) -> str:
    original = tomllib.loads(text)
    packages = original.get("package", [])
    owners = [p for p in packages if p.get("name") == "echo_tools"]
    if len(owners) != 1:
        raise ValueError("expected exactly one echo_tools owner in the actual lock")
    owner_source = owners[0].get("source")
    if owner_source is not None and (not isinstance(owner_source, str) or not owner_source.startswith("git+")):
        raise ValueError("echo_tools must be the path/Git framework owner")
    specs = {}
    for item in origins:
        name = item.get("name")
        if not isinstance(name, str) or name in specs:
            raise ValueError("missing or duplicate origin name")
        if not isinstance(item.get("version"), str):
            raise ValueError(f"missing origin version: {name}")
        if not isinstance(item.get("checksum"), str) or not re.fullmatch(r"[0-9a-f]{64}", item["checksum"]):
            raise ValueError(f"missing or invalid original registry checksum: {name}")
        matches = [p for p in packages if p.get("name") == name]
        if len(matches) != 1 or matches[0].get("version") != item["version"]:
            raise ValueError(f"missing, duplicate or wrong-version origin package: {name}")
        if matches[0].get("source") != owner_source:
            raise ValueError(f"origin source differs from echo_tools owner: {name}")
        if "checksum" in matches[0]:
            raise ValueError(f"unexpected checksum on path/Git origin package: {name}")
        specs[name] = item
    if not specs:
        raise ValueError("no registry origins configured")
    parts = re.split(r"(?m)(?=^\[\[package\]\]\r?$)", text)
    replaced = set()
    output = []
    for part in parts:
        block = tomllib.loads(part)
        entries = block.get("package", [])
        if not entries or entries[0].get("name") not in specs:
            output.append(part)
            continue
        package = entries[0]
        name = package["name"]
        spec = specs[name]
        lines = part.splitlines(keepends=True)
        version_lines = [i for i, line in enumerate(lines) if re.match(r"^version\s*=", line)]
        if len(version_lines) != 1:
            raise ValueError(f"unsupported lock serialization: {name}")
        lines = [line for line in lines if not re.match(r"^(source|checksum)\s*=", line)]
        index = next(i for i, line in enumerate(lines) if re.match(r"^version\s*=", line))
        newline = "\r\n" if lines[index].endswith("\r\n") else "\n"
        lines.insert(index + 1, f"source = {json.dumps(REGISTRY)}{newline}checksum = {json.dumps(spec['checksum'])}{newline}")
        output.append("".join(lines))
        replaced.add(name)
    if replaced != set(specs):
        raise ValueError("not all origins were projected")
    view = "".join(output)
    expected = tomllib.loads(text)
    for package in expected["package"]:
        if package.get("name") in specs:
            package["source"] = REGISTRY
            package["checksum"] = specs[package["name"]]["checksum"]
    if tomllib.loads(view) != expected:
        raise ValueError("audit view changed fields outside the declared source/checksum projection")
    return view

def load_origins(root: Path) -> list[dict]:
    meta = tomllib.loads((root / "vendor/registry-origins.toml").read_text(encoding="utf-8"))
    if set(meta) != {"schema_version", "package"} or meta["schema_version"] != 1 or len(meta["package"]) != 3:
        raise ValueError("expected the three declared media registry origins")
    for item in meta["package"]:
        if set(item) != {"name", "version", "checksum", "manifest"}:
            raise ValueError("unexpected registry-origin fields")
        manifest = root / item["manifest"]
        if manifest.is_symlink() or not manifest.resolve().is_relative_to((root / "vendor").resolve()):
            raise ValueError("origin manifest must belong to this framework's vendor directory")
        package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
        if package["name"] != item["name"] or package["version"] != item["version"]:
            raise ValueError("original origin name/version differs from the bundled manifest")
    return meta["package"]

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lockfile", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    lock = args.lockfile or root / "Cargo.lock"
    try:
        if args.output.resolve() == lock.resolve() or (args.output.exists() and args.output.samefile(lock)):
            raise ValueError("audit output must not overwrite the actual build lock")
        text = lock.read_bytes().decode("utf-8")
        view = registry_origin_view(text, load_origins(root))
        with args.output.open("x", encoding="utf-8", newline="") as out:
            out.write(view)
    except (ValueError, OSError, KeyError, TypeError) as error:
        print(f"registry-origin audit view rejected: {error}", file=sys.stderr)
        return 1
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
