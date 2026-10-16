from pathlib import Path
import copy
import json
import os
import re
import runpy
import subprocess
import sys
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/generate-vendored-media-audit-lock.py"
MODULE = runpy.run_path(str(SCRIPT))
SPECS = MODULE["load_origins"](ROOT)
REGISTRY = MODULE["REGISTRY"]
GIT = "git+https://example.invalid/framework#" + "1" * 40

def lock_text(source=None):
    source_line = f"source = {json.dumps(source)}\n" if source else ""
    result = 'version = 4\n\n[[package]]\nname = "echo_tools"\nversion = "0.2.0"\n' + source_line
    for spec in SPECS:
        result += f'\n[[package]]\nname = {json.dumps(spec["name"])}\nversion = {json.dumps(spec["version"])}\n' + source_line + 'dependencies = ["zip"]\n'
    return result + f'\n[[package]]\nname = "zip"\nversion = "9.0.3"\nsource = {json.dumps(REGISTRY)}\nchecksum = "' + '2' * 64 + '"\n'

def blocks(text):
    return re.split(r"(?m)(?=^\[\[package\]\]$)", text)

def change_parent(text, mutation):
    result = []
    for block in blocks(text):
        entries = tomllib.loads(block).get("package", [])
        if entries and entries[0]["name"] == SPECS[0]["name"]:
            block = mutation(block)
        result.append(block)
    return "".join(result)

class RegistryOriginGuards(unittest.TestCase):
    def test_path_and_git_preserve_every_other_package_field_and_edge(self):
        for source in [None, GIT]:
            with self.subTest(source=source):
                original = lock_text(source)
                specs = copy.deepcopy(SPECS)
                view = MODULE["registry_origin_view"](original, specs)
                expected = tomllib.loads(original)
                for item in expected["package"]:
                    match = next((s for s in SPECS if s["name"] == item["name"]), None)
                    if match:
                        item.update(source=REGISTRY, checksum=match["checksum"])
                self.assertEqual(tomllib.loads(view), expected)
                self.assertEqual(specs, SPECS)
                self.assertEqual(blocks(view)[-1], blocks(original)[-1])

    def test_unrelated_fork_or_missing_source_cannot_be_relabelled(self):
        for source in [None, GIT]:
            bad = change_parent(lock_text(source), lambda b: re.sub(r"(?m)^source = .*\n", "", b).replace('dependencies =', 'source = "git+https://unrelated.invalid/fork#' + '2' * 40 + '"\ndependencies ='))
            with self.subTest(source=source), self.assertRaisesRegex(ValueError, "source differs"):
                MODULE["registry_origin_view"](bad, SPECS)
        bad = change_parent(lock_text(GIT), lambda b: re.sub(r"(?m)^source = .*\n", "", b))
        with self.assertRaisesRegex(ValueError, "source differs"):
            MODULE["registry_origin_view"](bad, SPECS)

    def test_missing_parent_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "missing, duplicate"):
            MODULE["registry_origin_view"](change_parent(lock_text(), lambda b: ""), SPECS)

    def test_duplicate_parent_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "missing, duplicate"):
            MODULE["registry_origin_view"](change_parent(lock_text(), lambda b: b + b), SPECS)

    def test_wrong_version_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "wrong-version"):
            MODULE["registry_origin_view"](change_parent(lock_text(), lambda b: b.replace(SPECS[0]["version"], "0.0.0")), SPECS)

    def test_missing_invalid_checksum_and_duplicate_origins_are_rejected(self):
        for checksum in [None, "unknown", "X" * 64]:
            bad = copy.deepcopy(SPECS)
            bad[0]["checksum"] = checksum
            with self.subTest(checksum=checksum), self.assertRaisesRegex(ValueError, "checksum"):
                MODULE["registry_origin_view"](lock_text(), bad)
        with self.assertRaisesRegex(ValueError, "duplicate origin"):
            MODULE["registry_origin_view"](lock_text(), SPECS + [SPECS[0]])

    def test_missing_or_duplicate_owner_is_rejected(self):
        text = lock_text()
        owner = blocks(text)[1]
        for bad in [text.replace(owner, ""), text + owner]:
            with self.assertRaisesRegex(ValueError, "exactly one echo_tools"):
                MODULE["registry_origin_view"](bad, SPECS)

    def test_real_input_and_hardlink_output_are_not_overwritten(self):
        with tempfile.TemporaryDirectory() as tmp:
            lock = Path(tmp) / "Cargo.lock"
            lock.write_text(lock_text())
            alias = Path(tmp) / "alias.lock"
            os.link(lock, alias)
            before = lock.read_bytes()
            for output in [lock, alias]:
                result = subprocess.run([sys.executable, str(SCRIPT), "--lockfile", str(lock), "--output", str(output)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 1)
                self.assertIn("must not overwrite", result.stderr)
                self.assertEqual(lock.read_bytes(), before)

if __name__ == "__main__":
    unittest.main()
