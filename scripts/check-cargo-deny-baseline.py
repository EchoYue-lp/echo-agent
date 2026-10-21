"""Candidate exact diagnostic comparison; authorization is a separate human decision."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

SUMMARY = 'advisories ok, bans ok, licenses ok, sources ok'
HEADER = re.compile(r"warning\[duplicate\]: found ([0-9]+) duplicate entries for crate '([A-Za-z0-9_-]+)'")
ENTRY = re.compile(r'\s*[0-9]+ │ [╭│] ([A-Za-z0-9_-]+) ([^\s]+) ([^\s]+)')
LOCATION = re.compile(r' +┌─ [^\s]+/Cargo\.lock:[0-9]+:[0-9]+')
SPACER = re.compile(r' +│ *')
SEMVER = r'[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?'
TREE = re.compile(r'[ │]*[├└](?:──)? (?:\((?:build|dev)\) )?[A-Za-z0-9_-]+ v' + SEMVER + r'(?: \(\*\))?')
REQUIRED_RECEIPT = {'command', 'cwd', 'exitCode', 'signal', 'errorCode', 'truncated',
                    'bytesSeen', 'bytesWritten', 'rawSha256', 'toolVersion'}


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def exact_groups(raw: bytes) -> list[dict]:
    text = raw.decode('utf-8', errors='strict')
    lines = text.splitlines()
    if not text.endswith('\n') or not lines or lines[-1] != SUMMARY or lines.count(SUMMARY) != 1:
        raise ValueError('missing, repeated or incomplete four-check completion')
    groups = []
    current = None
    identities = []
    closed_entries = False

    def finish() -> None:
        if current is None:
            return
        count, name = current
        if not closed_entries or len(identities) != count or len(set(identities)) != count:
            raise ValueError('malformed or incomplete duplicate entries')
        groups.append({'name': name, 'identities': [
            {'version': version, 'source': source} for version, source in sorted(identities)]})

    for line in lines[:-1]:
        header = HEADER.fullmatch(line)
        if header:
            finish()
            current = (int(header[1]), header[2])
            if current[0] < 2:
                raise ValueError('invalid duplicate entry count')
            identities = []
            closed_entries = False
            continue
        entry = ENTRY.fullmatch(line)
        if entry:
            if current is None or closed_entries or entry[1] != current[1]:
                raise ValueError('unexpected diagnostic identity')
            identities.append((entry[2], entry[3]))
        elif 'lock entries' in line:
            if current is None or closed_entries or not re.fullmatch(r'\s*│ ╰─+┘ lock entries', line):
                raise ValueError('malformed duplicate entry closure')
            closed_entries = True
        elif not line:
            continue
        elif LOCATION.fullmatch(line) or SPACER.fullmatch(line) or TREE.fullmatch(line):
            if current is None:
                raise ValueError('rendered context outside a duplicate diagnostic')
        else:
            raise ValueError('unknown scanner rendering or diagnostic')
    finish()
    if len({g['name'] for g in groups}) != len(groups):
        raise ValueError('repeated duplicate group')
    return sorted(groups, key=lambda group: group['name'])


def normalized_command(command: list[str], root: Path, policy: bytes) -> list[str]:
    result = list(command)
    if '--config' in result:
        index = result.index('--config')
        if result.count('--config') != 1 or index + 1 >= len(result):
            raise ValueError('invalid config argument')
        path = Path(result[index + 1])
        effective = tomllib.loads((root / path).read_text())
        formal = tomllib.loads(policy.decode())
        advisory = effective.get('advisories', {})
        if not isinstance(advisory.get('db-path'), str):
            raise ValueError('derived config must declare its database path')
        del advisory['db-path']
        if effective != formal:
            raise ValueError('effective policy differs beyond database path')
        del result[index:index + 2]
    return result


def verify(raw: bytes, receipt: dict, baseline: dict, root: Path) -> dict:
    if not isinstance(receipt, dict) or not REQUIRED_RECEIPT.issubset(receipt):
        raise ValueError('missing required receipt facts; absence is not explicit null')
    cwd = receipt['cwd']
    if type(cwd) is not str or not cwd or not Path(cwd).is_absolute() or Path(cwd).resolve() != root.resolve():
        raise ValueError('receipt cwd differs from actual manifest root')
    if baseline.get('schema_version') != 1:
        raise ValueError('unknown baseline schema')
    if type(receipt.get('exitCode')) is not int or receipt['exitCode'] != 0:
        raise ValueError('scanner natural exit is not zero')
    if receipt.get('signal') is not None or receipt.get('errorCode') is not None:
        raise ValueError('scanner termination or execution error')
    if type(receipt.get('truncated')) is not bool or receipt['truncated']:
        raise ValueError('unknown or truncated raw output')
    for key in ('bytesSeen', 'bytesWritten'):
        if type(receipt.get(key)) is not int or receipt[key] < 0 or receipt[key] != len(raw):
            raise ValueError('raw byte counter mismatch')
    if receipt.get('rawSha256') != digest(raw):
        raise ValueError('raw content hash mismatch')
    if receipt.get('toolVersion') != baseline['tool_version']:
        raise ValueError('tool version changed')
    lock = (root / 'Cargo.lock').read_bytes()
    policy = (root / 'deny.toml').read_bytes()
    if digest(lock) != baseline['lock_sha256'] or digest(policy) != baseline['policy_sha256']:
        raise ValueError('lock or policy changed')
    command = receipt.get('command')
    if not isinstance(command, list) or not all(type(part) is str for part in command):
        raise ValueError('missing command scope')
    if normalized_command(command, root, policy) != baseline['command']:
        raise ValueError('scanner scope changed')
    expected = sorted([{'name': g['name'], 'identities': sorted(g['identities'], key=lambda x: (x['version'], x['source']))}
                       for g in baseline['groups']], key=lambda g: g['name'])
    if exact_groups(raw) != expected:
        raise ValueError('diagnostic groups, versions or sources differ from baseline')
    locked = {(p['name'], p['version'], p.get('source')) for p in tomllib.loads(lock.decode())['package']}
    if any((g['name'], p['version'], p['source']) not in locked for g in expected for p in g['identities']):
        raise ValueError('diagnostic identity absent from actual lock')
    return {'matches_exact_baseline': True, 'groups': len(expected), 'raw_sha256': digest(raw),
            'authorization': baseline['authorization']}


def capture(root: Path, baseline: dict, raw_path: Path, receipt_path: Path, config: Path | None = None) -> dict:
    # Candidate data cannot authorize a real scan. Original scanner output and exit are separate from this comparison.
    if baseline['authorization'] != 'ACCEPTED_BY_HUMAN':
        raise ValueError('human decision pending; capture forbidden')
    if raw_path.exists() or receipt_path.exists():
        raise ValueError('refusing to overwrite raw or receipt')
    version = subprocess.run(['cargo', 'deny', '--version'], cwd=root, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, check=False)
    if version.returncode != 0 or version.stdout.decode().strip() != baseline['tool_version']:
        raise ValueError('unexpected cargo-deny version')
    command = list(baseline['command'])
    if config is not None:
        command[-1:-1] = ['--config', str(config)]
    normalized_command(command, root, (root / 'deny.toml').read_bytes())
    receipt = {'command': command, 'cwd': str(root.resolve()), 'toolVersion': version.stdout.decode().strip(), 'exitCode': None,
               'signal': None, 'errorCode': None, 'truncated': False}
    try:
        with raw_path.open('xb') as output:
            result = subprocess.run(command, cwd=root, stdout=output, stderr=subprocess.STDOUT, check=False)
            output.flush()
            os.fsync(output.fileno())
        receipt['exitCode'] = result.returncode
        if result.returncode < 0:
            receipt['signal'] = -result.returncode
    except OSError as error:
        receipt['errorCode'] = str(error)
        receipt['truncated'] = True
    raw = raw_path.read_bytes() if raw_path.exists() else b''
    receipt.update(bytesSeen=len(raw), bytesWritten=len(raw), rawSha256=digest(raw))
    with receipt_path.open('x') as output:
        json.dump(receipt, output, indent=2)
        output.write('\n')
    return verify(raw, receipt, baseline, root)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--raw', type=Path, required=True)
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--capture', action='store_true')
    parser.add_argument('--config', type=Path)
    args = parser.parse_args()
    try:
        baseline = json.loads(args.baseline.read_text())
        result = capture(args.root, baseline, args.raw, args.receipt, args.config) if args.capture else verify(
            args.raw.read_bytes(), json.loads(args.receipt.read_text()), baseline, args.root)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(json.dumps({'matches_exact_baseline': False, 'error': str(error)}), file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
