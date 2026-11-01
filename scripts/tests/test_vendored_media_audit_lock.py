from pathlib import Path
import copy
import json
import os
import re
import runpy
import shutil
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

POLARS = [
    {"name": "polars-arrow", "version": "0.55.2", "checksum": "c5ab55460ca2553d1a2bbaf2046998cfbbb685bc18d1e168b4367b63301eb271", "manifest": "vendor/polars-arrow-0.55.2/Cargo.toml"},
    {"name": "polars-parquet", "version": "0.55.2", "checksum": "d33a17979d2afa0309437478662cf996361a6e1768bf467db82bd4f4a9443064", "manifest": "vendor/polars-parquet-0.55.2/Cargo.toml"},
]
MEDIA = [spec for spec in SPECS if spec["name"] not in {p["name"] for p in POLARS}]
FIVE_SPECS = MEDIA + POLARS

def lock_text(source=None, specs=None):
    source_line = f"source = {json.dumps(source)}\n" if source else ""
    result = 'version = 4\n\n[[package]]\nname = "echo_tools"\nversion = "0.2.0"\n' + source_line
    for spec in SPECS if specs is None else specs:
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
    def test_all_five_declarations_are_required_even_for_media_consumers(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for spec in SPECS:
                target = root / spec["manifest"]
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / spec["manifest"], target)
            path = root / "vendor/registry-origins.toml"
            original = (ROOT / "vendor/registry-origins.toml").read_text()
            path.write_text(original)
            self.assertEqual(MODULE["load_origins"](root), SPECS)
            for broken in [original.rsplit("[[package]]", 1)[0], original.replace('name = "polars-parquet"', 'name = "undeclared-parent"')]:
                path.write_text(broken)
                with self.assertRaises(ValueError):
                    MODULE["load_origins"](root)

    def test_duplicate_unselected_polars_declaration_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "duplicate origin"):
            MODULE["registry_origin_view"](lock_text(None, MEDIA), FIVE_SPECS + [POLARS[0]])

    def test_selected_polars_duplicate_version_and_source_errors_are_rejected(self):
        for spec in POLARS:
            original = lock_text(GIT, FIVE_SPECS)
            parent = next(block for block in blocks(original) if any(p.get("name") == spec["name"] for p in tomllib.loads(block).get("package", [])))
            for bad in [original + parent, original.replace(parent, parent.replace('version = "0.55.2"', 'version = "0.54.0"')), original.replace(parent, parent.replace(GIT, GIT + "other"))]:
                with self.subTest(package=spec["name"]), self.assertRaises(ValueError):
                    MODULE["registry_origin_view"](bad, FIVE_SPECS)

    def test_media_only_consumer_accepts_five_declarations_without_creating_polars(self):
        for source in [None, GIT]:
            with self.subTest(source=source):
                original = lock_text(source, MEDIA)
                view = MODULE["registry_origin_view"](original, FIVE_SPECS)
                projected = tomllib.loads(view)["package"]
                self.assertFalse(any(p["name"].startswith("polars-") for p in projected))
                for spec in MEDIA:
                    package = next(p for p in projected if p["name"] == spec["name"])
                    self.assertEqual(package["source"], REGISTRY)
                    self.assertEqual(package["checksum"], spec["checksum"])
                self.assertEqual(blocks(view)[-1], blocks(original)[-1])

    def test_arrow_only_consumer_uses_the_same_origin_selection(self):
        original = lock_text(GIT, MEDIA + [POLARS[0]])
        view = MODULE["registry_origin_view"](original, FIVE_SPECS)
        packages = tomllib.loads(view)["package"]
        arrow = next(p for p in packages if p["name"] == "polars-arrow")
        self.assertEqual(arrow["source"], REGISTRY)
        self.assertFalse(any(p["name"] == "polars-parquet" for p in packages))

    def test_referenced_missing_polars_cannot_be_treated_as_unselected(self):
        for missing in POLARS:
            original = lock_text(GIT, [spec for spec in FIVE_SPECS if spec != missing])
            original = original.replace('name = "echo_tools"\nversion = "0.2.0"', 'name = "echo_tools"\nversion = "0.2.0"\ndependencies = [' + json.dumps(missing["name"]) + ']')
            with self.subTest(missing=missing["name"]), self.assertRaisesRegex(ValueError, "missing, duplicate"):
                MODULE["registry_origin_view"](original, FIVE_SPECS)

    def test_origin_dependency_reference_must_match_declared_version(self):
        original = lock_text(GIT, FIVE_SPECS)
        original = original.replace('name = "echo_tools"\nversion = "0.2.0"', 'name = "echo_tools"\nversion = "0.2.0"\ndependencies = ["polars-arrow 0.54.0"]')
        with self.assertRaisesRegex(ValueError, "dependency.*version"):
            MODULE["registry_origin_view"](original, FIVE_SPECS)

    def test_origin_dependency_reference_cannot_point_to_another_source(self):
        original = lock_text(GIT, FIVE_SPECS)
        reference = "polars-arrow 0.55.2 (git+https://unrelated.invalid/other#" + "2" * 40 + ")"
        original = original.replace('name = "echo_tools"\nversion = "0.2.0"', 'name = "echo_tools"\nversion = "0.2.0"\ndependencies = [' + json.dumps(reference) + ']')
        with self.assertRaisesRegex(ValueError, "dependency.*source"):
            MODULE["registry_origin_view"](original, FIVE_SPECS)

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
