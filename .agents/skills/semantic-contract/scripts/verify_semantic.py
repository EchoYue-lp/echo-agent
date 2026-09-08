#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.9"
# dependencies = ["PyYAML>=6.0,<7"]
# ///
"""确定性校验仓库中的语义 Markdown 材料。"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import subprocess
import sys
import tempfile
from collections.abc import Iterable
from pathlib import Path
from typing import Any

import yaml

SCHEMA_VERSION = 1
KIND_BY_DIRECTORY = {
    "maps": "capability_map",
    "behaviors": "behavior",
    "rules": "rule",
    "evidence": "evidence",
    "findings": "finding",
    "audits": "audit",
    "discovery": "discovery",
}
REQUIRED_DIRECTORIES = tuple(KIND_BY_DIRECTORY)
RISK_VALUES = {"low", "medium", "high"}
LENS_VALUES = {
    "trigger_input",
    "result_side_effect",
    "state_authority",
    "data_durability",
    "time_lifecycle",
    "failure_concurrency",
    "permission_external",
    "contract_evidence",
}
REGION_VALUES = {"in_scope", "supporting", "generated_or_vendor", "excluded"}
COVERAGE_VALUES = {"covered", "not_applicable", "needs_review", "excluded"}
ID_PATTERN = re.compile(r"^[a-z0-9][a-z0-9.-]*$")
GIT_REVISION_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SOURCE_REVISION_PATTERN = re.compile(r"^source:[0-9a-f]{64}$")

REQUIRED_FIELDS = {
    "baseline": {
        "source_snapshot",
        "inventory_closure",
        "behavior_model_closure",
        "map_refs",
        "regions",
        "boundaries",
        "coverage",
    },
    "capability_map": {
        "title",
        "risk",
        "observed_at",
        "boundary_refs",
        "behavior_refs",
        "rule_refs",
        "evidence_refs",
        "finding_refs",
        "audit_refs",
        "related_map_refs",
        "scenarios",
    },
    "behavior": {
        "status",
        "expectation",
        "risk",
        "primary_focus",
        "focus",
        "boundary",
        "observed_at",
        "code_refs",
        "rule_refs",
        "evidence_refs",
        "finding_refs",
    },
    "rule": {
        "status",
        "expectation",
        "risk",
        "primary_focus",
        "focus",
        "observed_at",
        "behavior_refs",
        "code_refs",
        "evidence_refs",
        "finding_refs",
    },
    "evidence": {"observed_at", "source_refs", "supports", "limitations"},
    "finding": {
        "type",
        "status",
        "severity",
        "primary_focus",
        "focus",
        "boundary_ref",
        "behavior_refs",
        "rule_refs",
        "evidence_refs",
        "audit_refs",
        "decision_refs",
        "repair_evidence_refs",
        "verification_evidence_refs",
        "rereview_audit_refs",
        "discovered_at",
    },
    "audit": {
        "boundary_ref",
        "lens",
        "freshness",
        "revision",
        "challenges",
        "finding_refs",
    },
    "discovery": {
        "source_snapshot",
        "scope",
        "inspected_paths",
        "candidate_refs",
        "unresolved",
    },
}

REQUIRED_HEADINGS = {
    "baseline": {
        "源码快照",
        "仓库区域",
        "能力图与边界",
        "覆盖网格",
        "未知与缺口",
        "闭合结论",
    },
    "capability_map": {
        "能力范围",
        "入口与输出",
        "行为关系",
        "状态与数据流",
        "策略来源与优先级",
        "生命周期与失败路径",
        "权限与敏感信息",
        "用户侧投影",
        "场景处置清单",
        "未展开项",
    },
    "behavior": {
        "重要承诺",
        "当前行为",
        "期望行为",
        "触发、结果与副作用",
        "失败、重试与恢复",
        "证据",
        "裁决记录",
    },
    "rule": {
        "不变量或唯一权威",
        "适用行为",
        "当前实现",
        "期望行为",
        "证据",
        "裁决记录",
    },
    "evidence": {"支持的结论", "来源与范围", "已知缺口"},
    "finding": {"问题", "触发条件与影响", "证据", "处理记录"},
    "audit": {
        "审查范围",
        "已检查故障假设",
        "实际实现路径与证据",
        "问题记录",
        "残余风险",
        "未检查项",
    },
    "discovery": {"扫描范围", "候选事实", "归并结果", "未决项"},
}

SEMANTIC_REFERENCE_TYPES = {
    "map_refs": "capability_map",
    "related_map_refs": "capability_map",
    "behavior_refs": "behavior",
    "rule_refs": "rule",
    "evidence_refs": "evidence",
    "repair_evidence_refs": "evidence",
    "verification_evidence_refs": "evidence",
    "finding_refs": "finding",
    "audit_refs": "audit",
    "rereview_audit_refs": "audit",
}


def add_error(errors: list[str], path: Path, message: str) -> None:
    errors.append(f"{path}: {message}")


def split_document(path: Path, errors: list[str]) -> tuple[dict[str, Any], str] | None:
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as error:
        add_error(errors, path, f"无法读取：{error}")
        return None

    if not text.startswith("---\n"):
        add_error(errors, path, "缺少 YAML 前置元数据")
        return None
    end = text.find("\n---\n", 4)
    if end < 0:
        add_error(errors, path, "前置元数据没有结束标记")
        return None
    try:
        data = yaml.safe_load(text[4:end])
    except (yaml.YAMLError, RecursionError) as error:
        add_error(errors, path, f"YAML 无法解析：{error}")
        return None
    if not isinstance(data, dict):
        add_error(errors, path, "前置元数据必须是对象")
        return None
    return data, text[end + 5 :]


def source_revision_is_valid(value: Any) -> bool:
    return isinstance(value, str) and bool(
        GIT_REVISION_PATTERN.fullmatch(value)
        or SOURCE_REVISION_PATTERN.fullmatch(value)
    )


def require_string_list(
    data: dict[str, Any], field: str, path: Path, errors: list[str]
) -> list[str]:
    value = data.get(field)
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        add_error(errors, path, f"字段 {field} 必须是字符串列表")
        return []
    return value


def enum_is_valid(value: Any, allowed: set[str]) -> bool:
    return isinstance(value, str) and value in allowed


def string_items(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str)]


def object_items(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, dict)]


def mapping_items(value: Any) -> Iterable[tuple[Any, Any]]:
    if not isinstance(value, dict):
        return ()
    return value.items()


def mapping_values(value: Any) -> Iterable[Any]:
    if not isinstance(value, dict):
        return ()
    return value.values()


def validate_headings(kind: str, body: str, path: Path, errors: list[str]) -> None:
    actual = {line[3:].strip() for line in body.splitlines() if line.startswith("## ")}
    for heading in sorted(REQUIRED_HEADINGS.get(kind, set())):
        if heading not in actual:
            add_error(errors, path, f"缺少二级标题：{heading}")


def walk_scalars(value: Any) -> Iterable[Any]:
    pending = [value]
    visited: set[int] = set()
    while pending:
        item = pending.pop()
        if isinstance(item, dict):
            identity = id(item)
            if identity in visited:
                continue
            visited.add(identity)
            pending.extend(item.values())
        elif isinstance(item, list):
            identity = id(item)
            if identity in visited:
                continue
            visited.add(identity)
            pending.extend(item)
        else:
            yield item


def validate_common(
    data: dict[str, Any], kind: str, path: Path, errors: list[str]
) -> None:
    if data.get("schema_version") != SCHEMA_VERSION:
        add_error(errors, path, f"schema_version 必须是 {SCHEMA_VERSION}")
    if data.get("kind") != kind:
        add_error(errors, path, f"kind 必须是 {kind}")
    object_id = data.get("id")
    if not isinstance(object_id, str) or not ID_PATTERN.fullmatch(object_id):
        add_error(errors, path, "id 必须使用小写字母、数字、点和连字符")
    elif kind != "baseline" and path.stem != object_id:
        add_error(errors, path, "文件名必须等于对象 id")
    for field in sorted(REQUIRED_FIELDS[kind]):
        if field not in data:
            add_error(errors, path, f"缺少字段：{field}")
    for scalar in walk_scalars(data):
        if isinstance(scalar, str) and scalar.lower() in {"clean", "safe"}:
            add_error(errors, path, f"禁止绝对安全状态：{scalar}")


def validate_snapshot(value: Any, path: Path, errors: list[str]) -> None:
    if not isinstance(value, dict):
        add_error(errors, path, "source_snapshot 必须是对象")
        return
    base_revision = value.get("base_revision")
    content_digest = value.get("content_digest")
    if not isinstance(base_revision, str) or not GIT_REVISION_PATTERN.fullmatch(
        base_revision
    ):
        add_error(errors, path, "source_snapshot.base_revision 必须是 40 位 Git 提交")
    if not isinstance(content_digest, str) or not re.fullmatch(
        r"[0-9a-f]{64}", content_digest
    ):
        add_error(errors, path, "source_snapshot.content_digest 必须是 64 位 sha256")


def validate_baseline(data: dict[str, Any], path: Path, errors: list[str]) -> None:
    validate_snapshot(data.get("source_snapshot"), path, errors)
    for field in ("inventory_closure", "behavior_model_closure"):
        if not enum_is_valid(data.get(field), {"open", "closed"}):
            add_error(errors, path, f"{field} 只能是 open 或 closed")
    require_string_list(data, "map_refs", path, errors)

    regions = data.get("regions")
    if not isinstance(regions, list):
        add_error(errors, path, "regions 必须是列表")
        regions = []
    region_paths: set[str] = set()
    for region in regions:
        if not isinstance(region, dict):
            add_error(errors, path, "每个区域必须是对象")
            continue
        region_path = region.get("path")
        status = region.get("status")
        if not isinstance(region_path, str) or not region_path:
            add_error(errors, path, "区域缺少 path")
        else:
            if region_path in region_paths:
                add_error(errors, path, f"区域重复：{region_path}")
            region_paths.add(region_path)
        if not enum_is_valid(status, REGION_VALUES):
            add_error(errors, path, f"区域状态无效：{status}")
        if status == "excluded":
            for field in ("reason", "risk", "recheck_when"):
                if not region.get(field):
                    add_error(errors, path, f"排除区域缺少 {field}")
            if not enum_is_valid(region.get("risk"), RISK_VALUES):
                add_error(errors, path, f"排除区域风险无效：{region.get('risk')}")

    boundaries = data.get("boundaries")
    if not isinstance(boundaries, list):
        add_error(errors, path, "boundaries 必须是列表")
        boundaries = []
    boundary_ids: set[str] = set()
    for boundary in boundaries:
        if not isinstance(boundary, dict):
            add_error(errors, path, "每个边界必须是对象")
            continue
        boundary_id = boundary.get("id")
        if not isinstance(boundary_id, str) or not ID_PATTERN.fullmatch(boundary_id):
            add_error(errors, path, "边界 id 无效")
        elif boundary_id in boundary_ids:
            add_error(errors, path, f"边界重复：{boundary_id}")
        else:
            boundary_ids.add(boundary_id)
        if not isinstance(boundary.get("map_ref"), str):
            add_error(errors, path, f"边界 {boundary_id} 缺少 map_ref")
        if not enum_is_valid(boundary.get("risk"), RISK_VALUES):
            add_error(errors, path, f"边界 {boundary_id} 风险无效")

    coverage = data.get("coverage")
    if not isinstance(coverage, list):
        add_error(errors, path, "coverage 必须是列表")
        coverage = []
    coverage_keys: set[tuple[str, str]] = set()
    for cell in coverage:
        if not isinstance(cell, dict):
            add_error(errors, path, "每个覆盖格必须是对象")
            continue
        region = cell.get("region")
        lens = cell.get("lens")
        status = cell.get("status")
        if not isinstance(region, str) or region not in region_paths:
            add_error(errors, path, f"覆盖格引用未知区域：{region}")
        if not enum_is_valid(lens, LENS_VALUES):
            add_error(errors, path, f"覆盖格风险视角无效：{lens}")
        if not enum_is_valid(status, COVERAGE_VALUES):
            add_error(errors, path, f"覆盖格状态无效：{status}")
        key = (str(region), str(lens))
        if key in coverage_keys:
            add_error(errors, path, f"覆盖格重复：{region}/{lens}")
        coverage_keys.add(key)
        if status == "covered" and not cell.get("refs"):
            add_error(errors, path, f"covered 覆盖格缺少 refs：{region}/{lens}")
        if status in ("not_applicable", "excluded") and not cell.get("reason"):
            add_error(errors, path, f"{status} 覆盖格缺少 reason：{region}/{lens}")
        if status == "needs_review" and not cell.get("unknown"):
            add_error(errors, path, f"needs_review 覆盖格缺少 unknown：{region}/{lens}")

    if data.get("inventory_closure") == "closed":
        for region in regions:
            if isinstance(region, dict) and region.get("status") == "in_scope":
                for lens in LENS_VALUES:
                    if (str(region.get("path")), lens) not in coverage_keys:
                        add_error(
                            errors,
                            path,
                            f"闭合基线缺少覆盖格：{region.get('path')}/{lens}",
                        )


def validate_map(data: dict[str, Any], path: Path, errors: list[str]) -> None:
    if not enum_is_valid(data.get("risk"), RISK_VALUES):
        add_error(errors, path, "risk 无效")
    if not source_revision_is_valid(data.get("observed_at")):
        add_error(errors, path, "observed_at 不是有效源码版本")
    lists: dict[str, list[str]] = {}
    for field in (
        "boundary_refs",
        "behavior_refs",
        "rule_refs",
        "evidence_refs",
        "finding_refs",
        "audit_refs",
        "related_map_refs",
    ):
        lists[field] = require_string_list(data, field, path, errors)
    if not lists["behavior_refs"] and not lists["rule_refs"]:
        add_error(errors, path, "能力图必须引用至少一个行为承诺或规则")
    if not lists["boundary_refs"]:
        add_error(errors, path, "能力图必须拥有至少一个语义边界")
    scenarios = data.get("scenarios")
    if not isinstance(scenarios, dict):
        add_error(errors, path, "scenarios 必须是对象")
        return
    for name, scenario in scenarios.items():
        if not isinstance(scenario, dict):
            add_error(errors, path, f"场景 {name} 必须是对象")
            continue
        status = scenario.get("status")
        if not enum_is_valid(status, {"mapped", "needs_review", "excluded"}):
            add_error(errors, path, f"场景 {name} 状态无效")
        source_refs = scenario.get("source_refs")
        if not isinstance(source_refs, list) or not source_refs:
            add_error(errors, path, f"场景 {name} 缺少 source_refs")
        if status == "mapped" and not any(
            scenario.get(field)
            for field in ("behavior_refs", "rule_refs", "evidence_refs", "finding_refs")
        ):
            add_error(errors, path, f"mapped 场景 {name} 没有处置引用")
        for field in ("behavior_refs", "rule_refs", "evidence_refs", "finding_refs"):
            if field in scenario and (
                not isinstance(scenario[field], list)
                or any(not isinstance(item, str) for item in scenario[field])
            ):
                add_error(errors, path, f"场景 {name} 的 {field} 必须是字符串列表")
        if status == "needs_review" and (
            not scenario.get("unknown") or not scenario.get("next_step")
        ):
            add_error(
                errors, path, f"needs_review 场景 {name} 缺少 unknown 或 next_step"
            )
        if status == "excluded":
            for field in ("reason", "risk", "recheck_when"):
                if not scenario.get(field):
                    add_error(errors, path, f"excluded 场景 {name} 缺少 {field}")
            if not enum_is_valid(scenario.get("risk"), RISK_VALUES):
                add_error(errors, path, f"excluded 场景 {name} 的 risk 无效")


def validate_behavior_or_rule(
    data: dict[str, Any], kind: str, path: Path, errors: list[str]
) -> None:
    if not enum_is_valid(data.get("status"), {"needs_review", "verified", "stale"}):
        add_error(errors, path, "status 无效")
    if not enum_is_valid(
        data.get("expectation"), {"unknown", "inferred", "human_confirmed"}
    ):
        add_error(errors, path, "expectation 无效")
    if not enum_is_valid(data.get("risk"), RISK_VALUES):
        add_error(errors, path, "risk 无效")
    if not isinstance(data.get("primary_focus"), str) or not data.get("primary_focus"):
        add_error(errors, path, "primary_focus 必须是非空字符串")
    require_string_list(data, "focus", path, errors)
    if not source_revision_is_valid(data.get("observed_at")):
        add_error(errors, path, "observed_at 不是有效源码版本")
    if kind == "behavior" and not isinstance(data.get("boundary"), str):
        add_error(errors, path, "behavior 缺少 boundary")
    fields = ("code_refs", "rule_refs", "evidence_refs", "finding_refs")
    if kind == "rule":
        fields = ("behavior_refs", "code_refs", "evidence_refs", "finding_refs")
    for field in fields:
        require_string_list(data, field, path, errors)


def validate_evidence(data: dict[str, Any], path: Path, errors: list[str]) -> None:
    if not source_revision_is_valid(data.get("observed_at")):
        add_error(errors, path, "observed_at 不是有效源码版本")
    lists = {
        field: require_string_list(data, field, path, errors)
        for field in ("source_refs", "supports", "limitations")
    }
    if not lists["source_refs"]:
        add_error(errors, path, "独立证据必须有 source_refs")


def validate_finding(data: dict[str, Any], path: Path, errors: list[str]) -> None:
    if not enum_is_valid(
        data.get("type"),
        {
            "implementation_bug",
            "intent_gap",
            "evidence_gap",
            "authority_conflict",
        },
    ):
        add_error(errors, path, "type 无效")
    if not enum_is_valid(
        data.get("status"),
        {
            "open",
            "resolved",
            "risk_accepted",
            "false_positive",
        },
    ):
        add_error(errors, path, "status 无效")
    if not enum_is_valid(data.get("severity"), RISK_VALUES):
        add_error(errors, path, "severity 无效")
    if not isinstance(data.get("primary_focus"), str) or not data.get("primary_focus"):
        add_error(errors, path, "primary_focus 必须是非空字符串")
    require_string_list(data, "focus", path, errors)
    if not isinstance(data.get("boundary_ref"), str):
        add_error(errors, path, "boundary_ref 必须是字符串")
    if not source_revision_is_valid(data.get("discovered_at")):
        add_error(errors, path, "discovered_at 不是有效源码版本")
    for field in (
        "behavior_refs",
        "rule_refs",
        "evidence_refs",
        "audit_refs",
        "decision_refs",
        "repair_evidence_refs",
        "verification_evidence_refs",
        "rereview_audit_refs",
    ):
        require_string_list(data, field, path, errors)
    if data.get("type") == "implementation_bug" and not (
        data.get("evidence_refs") or data.get("audit_refs")
    ):
        add_error(errors, path, "implementation_bug 必须引用证据或审查")
    if data.get("status") == "resolved" and not all(
        data.get(field)
        for field in (
            "repair_evidence_refs",
            "verification_evidence_refs",
            "rereview_audit_refs",
        )
    ):
        add_error(errors, path, "resolved 必须有修复、验证和复审引用")
    if data.get("status") == "risk_accepted" and not data.get("decision_refs"):
        add_error(errors, path, "risk_accepted 必须有人的裁决引用")


def validate_audit(data: dict[str, Any], path: Path, errors: list[str]) -> None:
    if not enum_is_valid(data.get("lens"), LENS_VALUES):
        add_error(errors, path, "lens 无效")
    if not enum_is_valid(data.get("freshness"), {"examined", "stale"}):
        add_error(errors, path, "freshness 无效")
    if not source_revision_is_valid(data.get("revision")):
        add_error(errors, path, "revision 不是有效源码版本")
    if not isinstance(data.get("boundary_ref"), str):
        add_error(errors, path, "boundary_ref 必须是字符串")
    require_string_list(data, "finding_refs", path, errors)
    challenges = data.get("challenges")
    if not isinstance(challenges, dict) or not challenges:
        add_error(errors, path, "实际审查必须有非空 challenges")
        return
    for name, challenge in challenges.items():
        if not isinstance(challenge, dict):
            add_error(errors, path, f"故障假设 {name} 必须是对象")
            continue
        if not source_revision_is_valid(challenge.get("revision")):
            add_error(errors, path, f"故障假设 {name} 的 revision 无效")
        source_refs = challenge.get("source_refs")
        if (
            not isinstance(source_refs, list)
            or not source_refs
            or any(not isinstance(item, str) for item in source_refs)
        ):
            add_error(errors, path, f"故障假设 {name} 缺少 source_refs")
        evidence_refs = challenge.get("evidence_refs")
        if not isinstance(evidence_refs, list) or any(
            not isinstance(item, str) for item in evidence_refs
        ):
            add_error(errors, path, f"故障假设 {name} 的 evidence_refs 必须是列表")


def validate_discovery(data: dict[str, Any], path: Path, errors: list[str]) -> None:
    validate_snapshot(data.get("source_snapshot"), path, errors)
    for field in ("scope", "inspected_paths", "candidate_refs", "unresolved"):
        if field == "scope":
            if not isinstance(data.get(field), (str, list, dict)):
                add_error(errors, path, "scope 必须是字符串、列表或对象")
        else:
            require_string_list(data, field, path, errors)


def collect_documents(
    semantic_root: Path, errors: list[str]
) -> list[tuple[Path, str, dict[str, Any], str]]:
    documents: list[tuple[Path, str, dict[str, Any], str]] = []
    allowed_root_entries = {"README.md", "baseline.md", *REQUIRED_DIRECTORIES}
    try:
        root_entries = list(semantic_root.iterdir())
    except OSError as error:
        add_error(errors, semantic_root, f"无法读取语义目录：{error}")
        return documents
    for entry in root_entries:
        if entry.is_symlink():
            add_error(errors, entry, "语义目录根级条目不能使用符号链接")
            continue
        if entry.name not in allowed_root_entries:
            add_error(errors, entry, "语义目录包含合同未声明的根级条目")

    readme = semantic_root / "README.md"
    if not readme.is_file():
        add_error(errors, readme, "缺少 README.md")
    else:
        try:
            if not readme.read_text(encoding="utf-8").strip():
                add_error(errors, readme, "README.md 不能为空")
        except (OSError, UnicodeError) as error:
            add_error(errors, readme, f"无法读取：{error}")

    baseline = semantic_root / "baseline.md"
    if not baseline.is_file():
        add_error(errors, baseline, "缺少基线文件")
    else:
        parsed = split_document(baseline, errors)
        if parsed:
            documents.append((baseline, "baseline", parsed[0], parsed[1]))

    for directory, kind in KIND_BY_DIRECTORY.items():
        root = semantic_root / directory
        if not root.is_dir():
            add_error(errors, root, "缺少目录")
            continue
        try:
            entries = sorted(root.iterdir())
        except OSError as error:
            add_error(errors, root, f"无法读取目录：{error}")
            continue
        for path in entries:
            if path.is_symlink():
                add_error(errors, path, "语义对象不能使用符号链接")
                continue
            if path.is_dir():
                add_error(errors, path, "语义对象目录不允许嵌套子目录")
                continue
            if path.suffix != ".md":
                add_error(errors, path, "语义对象目录只能包含 Markdown 文件")
                continue
            parsed = split_document(path, errors)
            if parsed:
                documents.append((path, kind, parsed[0], parsed[1]))
    return documents


def validate_relations(
    documents: list[tuple[Path, str, dict[str, Any], str]], errors: list[str]
) -> None:
    objects: dict[str, tuple[str, dict[str, Any], Path]] = {}
    for path, kind, data, _ in documents:
        object_id = data.get("id")
        if not isinstance(object_id, str):
            continue
        if object_id in objects:
            add_error(errors, path, f"对象 id 重复：{object_id}")
        else:
            objects[object_id] = (kind, data, path)

    baseline_entry = next(
        (entry for entry in documents if entry[1] == "baseline"), None
    )
    boundary_to_map: dict[str, str] = {}
    boundary_risk: dict[str, str] = {}
    if baseline_entry:
        baseline_path, _, baseline, _ = baseline_entry
        map_ids = {
            object_id
            for object_id, (kind, _, _) in objects.items()
            if kind == "capability_map"
        }
        if set(string_items(baseline.get("map_refs"))) != map_ids:
            add_error(errors, baseline_path, "map_refs 与 maps/ 中对象集合不一致")
        for boundary in object_items(baseline.get("boundaries")):
            boundary_id = boundary.get("id")
            if not isinstance(boundary_id, str):
                continue
            boundary_to_map[boundary_id] = str(boundary.get("map_ref", ""))
            boundary_risk[boundary_id] = str(boundary.get("risk", ""))
            owner = objects.get(str(boundary.get("map_ref", "")))
            if owner is None or owner[0] != "capability_map":
                add_error(
                    errors,
                    baseline_path,
                    f"边界 {boundary_id} 引用不存在的能力图",
                )
            elif boundary_id not in string_items(owner[1].get("boundary_refs")):
                add_error(
                    errors,
                    baseline_path,
                    f"边界 {boundary_id} 未被其主要能力图引用",
                )

    for path, kind, data, _ in documents:
        for field, expected_kind in SEMANTIC_REFERENCE_TYPES.items():
            refs = data.get(field)
            if refs is None:
                continue
            for ref in string_items(refs):
                target = objects.get(ref)
                if target is None:
                    add_error(errors, path, f"{field} 引用不存在：{ref}")
                elif target[0] != expected_kind:
                    add_error(
                        errors,
                        path,
                        f"{field} 引用类型错误：{ref} 应为 {expected_kind}",
                    )

        if kind == "capability_map":
            map_id = data.get("id")
            for boundary in string_items(data.get("boundary_refs")):
                owner = boundary_to_map.get(boundary)
                if owner is None:
                    add_error(errors, path, f"引用未声明边界：{boundary}")
                elif owner != map_id:
                    add_error(
                        errors, path, f"边界 {boundary} 的主要能力图不是 {map_id}"
                    )
                elif (
                    enum_is_valid(data.get("risk"), RISK_VALUES)
                    and enum_is_valid(boundary_risk.get(boundary), RISK_VALUES)
                    and {"low": 0, "medium": 1, "high": 2}[data["risk"]]
                    < {"low": 0, "medium": 1, "high": 2}[boundary_risk[boundary]]
                ):
                    add_error(errors, path, f"能力图风险低于边界 {boundary}")
            for scenario_name, scenario in mapping_items(data.get("scenarios")):
                if not isinstance(scenario, dict):
                    continue
                for field, expected_kind in {
                    "behavior_refs": "behavior",
                    "rule_refs": "rule",
                    "evidence_refs": "evidence",
                    "finding_refs": "finding",
                }.items():
                    for ref in string_items(scenario.get(field)):
                        target = objects.get(ref)
                        if target is None:
                            add_error(
                                errors,
                                path,
                                f"场景 {scenario_name} 的 {field} 引用不存在：{ref}",
                            )
                        elif target[0] != expected_kind:
                            add_error(
                                errors,
                                path,
                                f"场景 {scenario_name} 的 {field} 引用类型错误：{ref}",
                            )
                        if ref not in string_items(data.get(field)):
                            add_error(
                                errors,
                                path,
                                f"场景 {scenario_name} 的 {field} 引用未进入能力图顶层索引：{ref}",
                            )
            if (
                baseline_entry
                and baseline_entry[2].get("behavior_model_closure") == "closed"
            ):
                scenarios = data.get("scenarios")
                if enum_is_valid(data.get("risk"), {"medium", "high"}) and (
                    not scenarios
                    or any(
                        isinstance(item, dict) and item.get("status") == "needs_review"
                        for item in mapping_values(scenarios)
                    )
                ):
                    add_error(
                        errors, path, "行为模型闭合时中高风险能力不能有未处置场景"
                    )

        if kind == "behavior":
            boundary = data.get("boundary")
            owner = boundary_to_map.get(boundary) if isinstance(boundary, str) else None
            if owner is None:
                add_error(errors, path, f"behavior 引用未声明边界：{boundary}")
            else:
                target = objects.get(owner)
                if target and data.get("id") not in string_items(
                    target[1].get("behavior_refs")
                ):
                    add_error(errors, path, f"行为承诺未被所属能力图 {owner} 引用")

        if kind in {"finding", "audit"}:
            boundary = data.get("boundary_ref")
            if not isinstance(boundary, str) or boundary not in boundary_to_map:
                add_error(errors, path, f"引用未声明边界：{boundary}")

        if kind == "audit":
            for challenge_name, challenge in mapping_items(data.get("challenges")):
                if not isinstance(challenge, dict):
                    continue
                for ref in string_items(challenge.get("evidence_refs")):
                    target = objects.get(ref)
                    if target is None:
                        add_error(
                            errors,
                            path,
                            f"故障假设 {challenge_name} 引用不存在证据：{ref}",
                        )
                    elif target[0] != "evidence":
                        add_error(
                            errors,
                            path,
                            f"故障假设 {challenge_name} 的证据引用类型错误：{ref}",
                        )


def validate_semantic_root(semantic_root: Path) -> list[str]:
    errors: list[str] = []
    if not semantic_root.is_dir():
        return [f"{semantic_root}: 语义目录不存在"]
    documents = collect_documents(semantic_root, errors)
    for path, kind, data, body in documents:
        validate_common(data, kind, path, errors)
        validate_headings(kind, body, path, errors)
        if kind == "baseline":
            validate_baseline(data, path, errors)
        elif kind == "capability_map":
            validate_map(data, path, errors)
        elif kind in {"behavior", "rule"}:
            validate_behavior_or_rule(data, kind, path, errors)
        elif kind == "evidence":
            validate_evidence(data, path, errors)
        elif kind == "finding":
            validate_finding(data, path, errors)
        elif kind == "audit":
            validate_audit(data, path, errors)
        elif kind == "discovery":
            validate_discovery(data, path, errors)
    validate_relations(documents, errors)
    return errors


def git_file_paths(repository_root: Path) -> list[str]:
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repository_root),
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        check=False,
        capture_output=True,
    )
    if result.returncode != 0:
        message = result.stderr.decode("utf-8", errors="replace").strip()
        raise RuntimeError(f"无法读取 Git 文件集合：{message}")
    paths = result.stdout.decode("utf-8", errors="strict").split("\0")
    return sorted(
        path
        for path in paths
        if path and not (path == "semantic" or path.startswith("semantic/"))
    )


def source_digest(repository_root: Path) -> str:
    digest = hashlib.sha256()
    for relative in git_file_paths(repository_root):
        path = repository_root / relative
        if path.is_symlink():
            identity = "symlink:" + os.readlink(path)
        elif path.is_file():
            identity = "file:" + hashlib.sha256(path.read_bytes()).hexdigest()
        else:
            continue
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(identity.encode("utf-8"))
        digest.update(b"\n")
    return digest.hexdigest()


def validate_strict_snapshot(repository_root: Path, semantic_root: Path) -> list[str]:
    errors: list[str] = []
    parsed = split_document(semantic_root / "baseline.md", errors)
    if not parsed:
        return errors
    snapshot = parsed[0].get("source_snapshot", {})
    try:
        head = subprocess.run(
            ["git", "-C", str(repository_root), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        digest = source_digest(repository_root)
    except (
        OSError,
        subprocess.CalledProcessError,
        RuntimeError,
        UnicodeError,
    ) as error:
        add_error(
            errors, semantic_root / "baseline.md", f"无法计算严格源码快照：{error}"
        )
        return errors
    if snapshot.get("base_revision") != head:
        add_error(
            errors,
            semantic_root / "baseline.md",
            f"基准提交不一致：材料={snapshot.get('base_revision')} 当前={head}",
        )
    if snapshot.get("content_digest") != digest:
        add_error(
            errors,
            semantic_root / "baseline.md",
            f"内容摘要不一致：材料={snapshot.get('content_digest')} 当前={digest}",
        )
    return errors


def write_document(path: Path, data: dict[str, Any], headings: Iterable[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    frontmatter = yaml.safe_dump(data, allow_unicode=True, sort_keys=False).rstrip()
    body = "\n\n".join(f"## {heading}\n\n已记录。" for heading in headings)
    path.write_text(
        f"---\n{frontmatter}\n---\n\n# 自测对象\n\n{body}\n", encoding="utf-8"
    )


def run_self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="semantic-verify-") as temp:
        repository = Path(temp) / "repository"
        repository.mkdir()
        subprocess.run(["git", "init", "-q", str(repository)], check=True)
        (repository / "source.txt").write_text("自测源码\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(repository), "add", "source.txt"], check=True)
        subprocess.run(
            [
                "git",
                "-C",
                str(repository),
                "-c",
                "user.name=Semantic Self Test",
                "-c",
                "user.email=semantic-self-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "test: 初始化语义校验样本",
            ],
            check=True,
        )
        revision = subprocess.run(
            ["git", "-C", str(repository), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        root = repository / "semantic"
        for directory in REQUIRED_DIRECTORIES:
            (root / directory).mkdir(parents=True, exist_ok=True)
        (root / "README.md").write_text("# 语义材料\n", encoding="utf-8")
        digest = source_digest(repository)
        observed = "source:" + digest
        coverage = [
            {
                "region": "src",
                "lens": lens,
                "status": "covered",
                "refs": ["behavior.example"],
            }
            for lens in sorted(LENS_VALUES)
        ]
        write_document(
            root / "baseline.md",
            {
                "schema_version": 1,
                "id": "baseline.repository",
                "kind": "baseline",
                "source_snapshot": {
                    "base_revision": revision,
                    "content_digest": digest,
                },
                "inventory_closure": "closed",
                "behavior_model_closure": "closed",
                "map_refs": ["map.example"],
                "regions": [{"path": "src", "status": "in_scope"}],
                "boundaries": [
                    {"id": "boundary.example", "map_ref": "map.example", "risk": "high"}
                ],
                "coverage": coverage,
            },
            REQUIRED_HEADINGS["baseline"],
        )
        write_document(
            root / "maps" / "map.example.md",
            {
                "schema_version": 1,
                "id": "map.example",
                "kind": "capability_map",
                "title": "示例能力",
                "risk": "high",
                "observed_at": observed,
                "boundary_refs": ["boundary.example"],
                "behavior_refs": ["behavior.example"],
                "rule_refs": [],
                "evidence_refs": [],
                "finding_refs": [],
                "audit_refs": [],
                "related_map_refs": [],
                "scenarios": {
                    "example": {
                        "status": "mapped",
                        "source_refs": ["src/lib.rs#example"],
                        "behavior_refs": ["behavior.example"],
                    }
                },
            },
            REQUIRED_HEADINGS["capability_map"],
        )
        write_document(
            root / "behaviors" / "behavior.example.md",
            {
                "schema_version": 1,
                "id": "behavior.example",
                "kind": "behavior",
                "status": "verified",
                "expectation": "inferred",
                "risk": "high",
                "primary_focus": "time_lifecycle",
                "focus": ["state_authority"],
                "boundary": "boundary.example",
                "observed_at": observed,
                "code_refs": ["src/lib.rs#example"],
                "rule_refs": [],
                "evidence_refs": [],
                "finding_refs": [],
            },
            REQUIRED_HEADINGS["behavior"],
        )
        errors = validate_semantic_root(root)
        if errors:
            print("自测中的有效样本被错误拒绝：", file=sys.stderr)
            print("\n".join(errors), file=sys.stderr)
            return 1
        errors = validate_strict_snapshot(repository, root)
        if errors:
            print("自测中的严格源码快照被错误拒绝：", file=sys.stderr)
            print("\n".join(errors), file=sys.stderr)
            return 1
        behavior = root / "behaviors" / "behavior.example.md"
        behavior.write_text(
            behavior.read_text(encoding="utf-8").replace(
                "status: verified", "status: safe"
            ),
            encoding="utf-8",
        )
        errors = validate_semantic_root(root)
        if not any(
            "禁止绝对安全状态" in error or "status 无效" in error for error in errors
        ):
            print("自测中的无效样本未被拒绝", file=sys.stderr)
            return 1
        behavior.write_text(
            behavior.read_text(encoding="utf-8").replace(
                "status: safe", "status: verified"
            ),
            encoding="utf-8",
        )
        baseline = root / "baseline.md"
        baseline.write_text(
            baseline.read_text(encoding="utf-8").replace(
                "- map.example", "- map.missing", 1
            ),
            encoding="utf-8",
        )
        errors = validate_semantic_root(root)
        if not any("map_refs 与 maps/ 中对象集合不一致" in error for error in errors):
            print("自测中的悬空能力图引用未被拒绝", file=sys.stderr)
            return 1
        baseline.write_text(
            baseline.read_text(encoding="utf-8").replace("- map.missing", "- []", 1),
            encoding="utf-8",
        )
        errors = validate_semantic_root(root)
        if not any("字段 map_refs 必须是字符串列表" in error for error in errors):
            print("自测中的畸形引用列表未被稳定拒绝", file=sys.stderr)
            return 1
        baseline.write_text(
            baseline.read_text(encoding="utf-8").replace("- []", "- map.example", 1),
            encoding="utf-8",
        )
        nested = root / "maps" / "nested"
        nested.mkdir()
        (nested / "hidden.md").write_text("# 不应被忽略\n", encoding="utf-8")
        errors = validate_semantic_root(root)
        if not any("不允许嵌套子目录" in error for error in errors):
            print("自测中的嵌套语义对象未被拒绝", file=sys.stderr)
            return 1
        invalid_utf8 = root / "evidence" / "evidence.invalid.md"
        invalid_utf8.write_bytes(b"\xff\xfe")
        errors = validate_semantic_root(root)
        if not any(
            "无法读取" in error and "evidence.invalid.md" in error for error in errors
        ):
            print("自测中的非法 UTF-8 对象未被稳定拒绝", file=sys.stderr)
            return 1
    print("语义结构校验器自测通过")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="校验语义 Markdown 材料")
    parser.add_argument("--root", type=Path, help="仓库根或 semantic 目录")
    parser.add_argument(
        "--strict-snapshot", action="store_true", help="同时校验当前 Git 内容摘要"
    )
    parser.add_argument(
        "--self-test", action="store_true", help="运行内置有效与无效样本"
    )
    args = parser.parse_args()

    if args.self_test:
        if args.root is not None or args.strict_snapshot:
            parser.error("--self-test 不能与其它参数同时使用")
        return run_self_test()
    if args.root is None:
        parser.error("必须提供 --root 或 --self-test")

    supplied = args.root.expanduser().resolve()
    semantic_root = supplied if supplied.name == "semantic" else supplied / "semantic"
    repository_root = semantic_root.parent
    errors = validate_semantic_root(semantic_root)
    if args.strict_snapshot and not errors:
        errors.extend(validate_strict_snapshot(repository_root, semantic_root))
    if errors:
        print(f"语义材料校验失败，共 {len(errors)} 项：", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(f"语义材料结构校验通过：{semantic_root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
