"""Self-contained rendering fixtures and fake-process controls; never execute a scanner."""
from __future__ import annotations

import copy
import json
import os
import subprocess
import sys
from pathlib import Path
import re
import shutil
import tempfile
import types
import unittest
from unittest.mock import patch

import importlib.util

SCRIPT = Path(__file__).resolve().parents[1] / 'check-cargo-deny-baseline.py'
SPEC = importlib.util.spec_from_file_location('cargo_deny_baseline', SCRIPT)
checker = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(checker)

REGISTRY = 'registry+https://github.com/rust-lang/crates.io-index'
# A deliberately small literal rendering fixture, not a claim about any real scanner run.
FIXTURE = """warning[duplicate]: found 2 duplicate entries for crate 'block-buffer'
   ┌─ /fixture/Cargo.lock:1:1
   │\x20\x20
1 │ ╭ block-buffer 0.10.4 registry+https://github.com/rust-lang/crates.io-index
2 │ │ block-buffer 0.12.1 registry+https://github.com/rust-lang/crates.io-index
   │ ╰─────────────────────────────────────────────────────────────────────────┘ lock entries
   │\x20\x20
   ├ block-buffer v0.10.4
     └── fixture v1.0.0

warning[duplicate]: found 2 duplicate entries for crate 'schemars'
   ┌─ /fixture/Cargo.lock:3:1
   │\x20\x20
3 │ ╭ schemars 0.8.22 registry+https://github.com/rust-lang/crates.io-index
4 │ │ schemars 1.2.2 registry+https://github.com/rust-lang/crates.io-index
   │ ╰─────────────────────────────────────────────────────────────────────────┘ lock entries
   │\x20\x20
   ├ schemars v0.8.22
     └── fixture v1.0.0

advisories ok, bans ok, licenses ok, sources ok
""".encode()


class ExactBaselineTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        packages = [('block-buffer', '0.10.4'), ('block-buffer', '0.12.1'),
                    ('schemars', '0.8.22'), ('schemars', '0.9.0'), ('schemars', '1.2.2')]
        lock = 'version = 3\n' + ''.join(
            f'[[package]]\nname = "{name}"\nversion = "{version}"\nsource = "{REGISTRY}"\n'
            for name, version in packages)
        policy = '[advisories]\n[bans]\nmultiple-versions = "warn"\n'
        (self.root / 'Cargo.lock').write_text(lock)
        (self.root / 'deny.toml').write_text(policy)
        self.raw = FIXTURE
        self.baseline = {
            'schema_version': 1, 'authorization': 'PROPOSAL_ONLY_PENDING_HUMAN_DECISION',
            'lock_sha256': checker.digest(lock.encode()), 'policy_sha256': checker.digest(policy.encode()),
            'tool_version': 'cargo-deny 0.20.2',
            'command': ['cargo', 'deny', '--log-level', 'warn', '--manifest-path', './Cargo.toml',
                        '--workspace', '--all-features', '--locked', 'check'],
            'groups': [{'name': 'block-buffer', 'identities': [
                {'version': '0.10.4', 'source': REGISTRY}, {'version': '0.12.1', 'source': REGISTRY}]},
                {'name': 'schemars', 'identities': [
                {'version': '0.8.22', 'source': REGISTRY}, {'version': '1.2.2', 'source': REGISTRY}]}]}
        config = self.root / 'effective.toml'
        config.write_text(policy.replace('[advisories]', '[advisories]\ndb-path = "/fixture/advisory-db"'))
        command = list(self.baseline['command'])
        command[-1:-1] = ['--config', str(config)]
        self.receipt = {'command': command, 'cwd': str(self.root.resolve()), 'exitCode': 0,
                        'signal': None, 'errorCode': None, 'truncated': False,
                        'bytesSeen': len(self.raw), 'bytesWritten': len(self.raw),
                        'rawSha256': checker.digest(self.raw), 'toolVersion': 'cargo-deny 0.20.2'}

    def verify(self, raw=None, receipt=None, baseline=None):
        return checker.verify(self.raw if raw is None else raw, self.receipt if receipt is None else receipt,
                              self.baseline if baseline is None else baseline, self.root)

    def updated_receipt(self, raw):
        receipt = copy.deepcopy(self.receipt)
        receipt.update(rawSha256=checker.digest(raw), bytesSeen=len(raw), bytesWritten=len(raw))
        return receipt

    def reject_changed_raw(self, raw):
        with self.assertRaises(ValueError):
            self.verify(raw, self.updated_receipt(raw))

    def test_literal_fixture_matches_without_authorizing_real_scans(self):
        result = self.verify()
        self.assertEqual(result['groups'], 2)
        self.assertEqual(result['authorization'], 'PROPOSAL_ONLY_PENDING_HUMAN_DECISION')
        actual = next(g for g in checker.exact_groups(self.raw) if g['name'] == 'schemars')
        self.assertEqual([p['version'] for p in actual['identities']], ['0.8.22', '1.2.2'])
        self.assertIn('version = "0.9.0"', (self.root / 'Cargo.lock').read_text())

    def test_new_group_same_count_and_missing_group_rejected(self):
        first_end = self.raw.index(b'warning[duplicate]', 1)
        # A well-formed replacement keeps 2 groups and the original entry count.
        self.reject_changed_raw(self.raw[:first_end].replace(b'block-buffer', b'new-crate') + self.raw[first_end:])
        self.reject_changed_raw(self.raw[first_end:])
        self.reject_changed_raw(self.raw[:first_end] + self.raw)

    def test_new_version_and_source_rejected(self):
        self.reject_changed_raw(self.raw.replace(b'block-buffer 0.10.4 registry+', b'block-buffer 9.10.4 registry+', 1))
        self.reject_changed_raw(self.raw.replace(b'registry+https://github.com/rust-lang/crates.io-index',
                                                b'registry+https://unaccepted.example/index', 1))

    def test_other_warning_error_and_unknown_output_rejected(self):
        for line in (b'warning[unmaintained]: new warning\n', b'error[advisory]: error\n',
                     b'warning: unknown\n', b'unknown scanner output\n', b'  error: indented\n'):
            with self.subTest(line=line):
                self.reject_changed_raw(line + self.raw)

    def test_truncated_and_damaged_diagnostics_rejected(self):
        for raw in (self.raw[:-1], self.raw[:-60], self.raw.replace(b'lock entries', b'broken entries', 1),
                    self.raw.replace(b'found 2 duplicate', b'found 3 duplicate', 1),
                    b'\xff' + self.raw):
            with self.subTest(raw_bytes=len(raw)):
                self.reject_changed_raw(raw)

    def test_receipt_byte_integrity_and_strict_types(self):
        for key, value in (('bytesSeen', True), ('bytesWritten', -1), ('bytesSeen', len(self.raw) + 1),
                           ('truncated', 0), ('truncated', True), ('rawSha256', 'bad'),
                           ('exitCode', False), ('exitCode', 101), ('exitCode', -2),
                           ('signal', 2), ('errorCode', 'UNKNOWN')):
            with self.subTest(key=key, value=value):
                receipt = copy.deepcopy(self.receipt)
                receipt[key] = value
                with self.assertRaises(ValueError):
                    self.verify(receipt=receipt)

    def test_scope_tool_policy_and_lock_changes_rejected(self):
        for key, value in (('toolVersion', 'cargo-deny 0.20.3'), ('command', ['cargo', 'deny', 'check'])):
            receipt = copy.deepcopy(self.receipt)
            receipt[key] = value
            with self.assertRaises(ValueError):
                self.verify(receipt=receipt)
        for name in ('Cargo.lock', 'deny.toml'):
            original = (self.root / name).read_bytes()
            (self.root / name).write_bytes(original + b'\n')
            with self.assertRaises(ValueError):
                self.verify()
            (self.root / name).write_bytes(original)
        config = self.root / 'effective.toml'
        config.write_text(config.read_text().replace('multiple-versions = "warn"', 'multiple-versions = "allow"'))
        with self.assertRaises(ValueError):
            self.verify()

    def test_unknown_indented_rendering_rejected(self):
        for prefix in (b'  warn[new-category]: hidden\n', b'  \xe2\x94\x82 warning[unmaintained]: hidden\n',
                       b'  unexpected scanner output\n'):
            with self.subTest(prefix=prefix):
                self.reject_changed_raw(prefix + self.raw)
                first_line = self.raw.index(b'\n') + 1
                self.reject_changed_raw(self.raw[:first_line] + prefix + self.raw[first_line:])

    def test_cwd_and_explicit_null_receipt_fields_required(self):
        for key, value in (('cwd', '/different/repository'), ('cwd', None),
                           ('signal', 'MISSING'), ('errorCode', 'MISSING'), ('cwd', 'MISSING')):
            with self.subTest(key=key, value=value):
                receipt = copy.deepcopy(self.receipt)
                if value == 'MISSING':
                    del receipt[key]
                else:
                    receipt[key] = value
                with self.assertRaises(ValueError):
                    self.verify(receipt=receipt)

    def test_actual_capture_forbidden_while_human_decision_pending(self):
        with patch.object(checker.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'pending'):
                checker.capture(self.root, self.baseline, self.root / 'raw', self.root / 'receipt')
            run.assert_not_called()

    def fake_capture(self, exit_code):
        baseline = copy.deepcopy(self.baseline)
        baseline['authorization'] = 'ACCEPTED_BY_HUMAN'  # fixture authority only, not real acceptance
        calls = []

        def fake_run(command, **kwargs):
            calls.append(command)
            if command == ['cargo', 'deny', '--version']:
                return types.SimpleNamespace(returncode=0, stdout=b'cargo-deny 0.20.2\n')
            self.assertEqual(command, self.baseline['command'])
            kwargs['stdout'].write(self.raw)
            return types.SimpleNamespace(returncode=exit_code)

        raw_path, receipt_path = self.root / 'captured.raw', self.root / 'captured.receipt'
        with patch.object(checker.subprocess, 'run', side_effect=fake_run):
            if exit_code == 0:
                self.assertTrue(checker.capture(self.root, baseline, raw_path, receipt_path)['matches_exact_baseline'])
            else:
                with self.assertRaises(ValueError):
                    checker.capture(self.root, baseline, raw_path, receipt_path)
        self.assertEqual(raw_path.read_bytes(), self.raw)
        receipt = json.loads(receipt_path.read_text())
        self.assertEqual(receipt['exitCode'], exit_code)
        self.assertEqual(receipt['cwd'], str(self.root.resolve()))
        self.assertEqual(receipt['rawSha256'], checker.digest(self.raw))
        self.assertEqual(len(calls), 2)

    def test_fake_capture_preserves_natural_zero_and_raw(self):
        self.fake_capture(0)

    def test_fake_capture_preserves_101_and_all_original_raw(self):
        self.fake_capture(101)

    def test_fake_capture_preserves_signal_not_success(self):
        self.fake_capture(-2)


    def test_capture_plain_child_rendering_preserves_ambient_parent_color(self):
        baseline = copy.deepcopy(self.baseline)
        baseline['authorization'] = 'ACCEPTED_BY_HUMAN'  # fixture only
        fixture = self.root / 'literal.raw'
        fixture.write_bytes(self.raw)
        renderer = self.root / 'fake_cargo_renderer.py'
        renderer.write_text("""import json, os, pathlib, sys
root = pathlib.Path.cwd()
trace = root / 'render-env.jsonl'
with trace.open('a') as output:
    output.write(json.dumps({'argv': sys.argv[1:],
                             'color': os.environ.get('CARGO_TERM_COLOR')}) + '\\n')
if sys.argv[1:] == ['deny', '--version']:
    sys.stdout.buffer.write(b'cargo-deny 0.20.2\\n')
else:
    raw = (root / 'literal.raw').read_bytes()
    if os.environ.get('CARGO_TERM_COLOR') == 'always':
        raw = raw.replace(b' ok', b' \\x1b[32mok\\x1b[0m')
    sys.stdout.buffer.write(raw)
""")
        real_run = subprocess.run
        calls = []

        def render_in_real_python_child(command, **kwargs):
            calls.append(command)
            self.assertIn(command, [['cargo', 'deny', '--version'], baseline['command']])
            return real_run([sys.executable, '-B', str(renderer), *command[1:]], **kwargs)

        raw_path = self.root / 'ambient-color.raw'
        receipt_path = self.root / 'ambient-color.receipt.json'
        with patch.dict(os.environ, {'CARGO_TERM_COLOR': 'always'}):
            with patch.object(checker.subprocess, 'run', side_effect=render_in_real_python_child):
                result = checker.capture(self.root, baseline, raw_path, receipt_path)
            self.assertEqual(os.environ['CARGO_TERM_COLOR'], 'always')
        self.assertTrue(result['matches_exact_baseline'])
        self.assertEqual(checker.exact_groups(raw_path.read_bytes()), checker.exact_groups(self.raw))
        self.assertEqual(raw_path.read_bytes(), self.raw)
        receipt = json.loads(receipt_path.read_text())
        self.assertEqual(receipt['command'], baseline['command'])
        self.assertEqual(receipt['exitCode'], 0)
        self.assertEqual(receipt['rawSha256'], checker.digest(self.raw))
        self.assertEqual(receipt['bytesSeen'], len(self.raw))
        self.assertEqual(calls, [['cargo', 'deny', '--version'], baseline['command']])
        trace = [json.loads(line) for line in (self.root / 'render-env.jsonl').read_text().splitlines()]
        self.assertEqual([entry['color'] for entry in trace], ['never', 'never'])
        self.assertEqual([entry['argv'] for entry in trace], [call[1:] for call in calls])

if __name__ == '__main__':
    unittest.main(verbosity=2)
