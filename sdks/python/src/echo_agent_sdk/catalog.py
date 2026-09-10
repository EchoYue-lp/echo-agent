from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True, slots=True)
class ResolvedOperation:
    operation: str
    family: str
    method: str
    signature: str


@dataclass(frozen=True, slots=True)
class CatalogFamily:
    """A deterministic family projection of the canonical operation catalog."""

    family: str
    methods: tuple[str, ...]
    operations: tuple[ResolvedOperation, ...]


class FacadeCatalog:
    def __init__(self, path: str | Path | None = None) -> None:
        resolved = (
            Path(path)
            if path is not None
            else Path(__file__).resolve().parents[3]
            / "shared"
            / "facade-operation-catalog.json"
        )
        self.document: dict[str, Any] = json.loads(resolved.read_text(encoding="utf-8"))
        digest_path = resolved.with_name("contract-digests.json")
        digests = json.loads(digest_path.read_text(encoding="utf-8"))
        self.contract_digest = str(digests["contract_digest"])
        self.source_contract_digest = str(digests["source_contract_digest"])
        routes = self.document.get("routes", [])
        self._operations = {
            route["operation"]: route for route in routes if route.get("operation")
        }
        self._family_routes: dict[str, list[dict[str, Any]]] = {}
        for route in routes:
            method = route.get("method")
            if route.get("operation") is None and method:
                self._family_routes.setdefault(str(method), []).append(route)

        operation_entries: list[ResolvedOperation] = []
        for route in routes:
            operation = route.get("operation")
            if operation:
                operation_entries.append(self._resolved_route(route, str(operation)))
            method = route.get("method")
            if not method:
                continue
            for item in route.get("operation_signatures", []):
                nested_operation = item.get("operation")
                if nested_operation:
                    operation_entries.append(
                        self._resolved_family_operation(
                            route, item, str(method), str(nested_operation)
                        )
                    )

        operation_entries.sort(
            key=lambda item: (item.operation, item.family, item.method, item.signature)
        )
        identities = [
            (item.operation, item.family, item.method, item.signature)
            for item in operation_entries
        ]
        if len(identities) != len(set(identities)):
            raise ValueError(
                "canonical catalog contains duplicate operation identities"
            )
        self._operation_entries = tuple(operation_entries)

        grouped_operations: dict[str, list[ResolvedOperation]] = {}
        for item in self._operation_entries:
            grouped_operations.setdefault(item.family, []).append(item)
        family_entries: list[CatalogFamily] = []
        family_names = {
            str(item["family"])
            for item in self.document.get("families", [])
            if item.get("family")
        }
        family_names.update(grouped_operations)
        for family_name in sorted(family_names):
            metadata = next(
                (
                    item
                    for item in self.document.get("families", [])
                    if item.get("family") == family_name
                ),
                {},
            )
            methods = {str(method) for method in metadata.get("methods", []) if method}
            methods.update(
                item.method for item in grouped_operations.get(family_name, [])
            )
            family_entries.append(
                CatalogFamily(
                    family=family_name,
                    methods=tuple(sorted(methods)),
                    operations=tuple(grouped_operations.get(family_name, [])),
                )
            )
        self._family_entries = tuple(family_entries)

    @staticmethod
    def _signature(route: dict[str, Any], context: str) -> str:
        digests = route.get("signature_digests", [])
        if not digests:
            raise ValueError(f"canonical catalog route has no signature: {context}")
        return str(digests[0])

    @classmethod
    def _resolved_route(
        cls, route: dict[str, Any], operation: str
    ) -> ResolvedOperation:
        family = route.get("family")
        method = route.get("method")
        if not family or not method:
            raise ValueError(
                f"canonical source route is missing family/method: {operation}"
            )
        return ResolvedOperation(
            operation,
            str(family),
            str(method),
            cls._signature(route, operation),
        )

    @classmethod
    def _resolved_family_operation(
        cls,
        route: dict[str, Any],
        item: dict[str, Any],
        method: str,
        operation: str,
    ) -> ResolvedOperation:
        family = route.get("family")
        digests = item.get("signature_digests", [])
        if not family or not digests:
            raise ValueError(
                f"canonical family route is missing family/signature: {method}/{operation}"
            )
        return ResolvedOperation(operation, str(family), method, str(digests[0]))

    def operations(self) -> tuple[ResolvedOperation, ...]:
        """Return every source and family operation in canonical order."""

        return self._operation_entries

    def source_operations(self) -> tuple[ResolvedOperation, ...]:
        """Return source-operation routes in canonical order."""

        return tuple(
            item
            for item in self._operation_entries
            if item.family == "source_operation"
        )

    def family_operations(self) -> tuple[ResolvedOperation, ...]:
        """Return nested family-operation routes in canonical order."""

        return tuple(
            item
            for item in self._operation_entries
            if item.family != "source_operation"
        )

    def families(self) -> tuple[CatalogFamily, ...]:
        """Return all canonical families, including families with no operations."""

        return self._family_entries

    def signature(self, operation: str) -> str:
        route = self._operations.get(operation)
        digests = route.get("signature_digests", []) if route else []
        if not digests:
            raise KeyError(f"operation is absent from canonical catalog: {operation}")
        return str(digests[0])

    def family_signature(self, method: str, operation: str) -> str:
        for route in self._family_routes.get(method, []):
            for item in route.get("operation_signatures", []):
                if item.get("operation") == operation and item.get("signature_digests"):
                    return str(item["signature_digests"][0])
        raise KeyError(
            f"family operation is absent from canonical catalog: {method}/{operation}"
        )

    def resolve(self, operation: str) -> ResolvedOperation:
        direct = self._operations.get(operation)
        if direct and direct.get("method") and direct.get("signature_digests"):
            return ResolvedOperation(
                operation,
                str(direct["family"]),
                str(direct["method"]),
                str(direct["signature_digests"][0]),
            )
        for method, routes in self._family_routes.items():
            for route in routes:
                for item in route.get("operation_signatures", []):
                    if item.get("operation") == operation and item.get(
                        "signature_digests"
                    ):
                        return ResolvedOperation(
                            operation,
                            str(route["family"]),
                            method,
                            str(item["signature_digests"][0]),
                        )
        raise KeyError(f"operation is absent from the canonical catalog: {operation}")
