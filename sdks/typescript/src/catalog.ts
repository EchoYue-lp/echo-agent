import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export interface CatalogRoute {
  readonly family: string;
  readonly method: string | null;
  readonly operation: string | null;
  readonly handler_operation?: string | null;
  readonly route: string;
  readonly signature_digests?: readonly string[];
  readonly operation_signatures?: readonly {
    readonly operation: string;
    readonly signature_digests: readonly string[];
  }[];
  readonly required_feature?: string | null;
  readonly required_features?: readonly string[];
  readonly feature_semantics?: string;
}

export interface ResolvedOperation {
  readonly operation: string;
  readonly family: string;
  readonly method: string;
  readonly signature: string;
}

export interface CatalogOperation {
  /** Stable identity scoped by the catalog family. */
  readonly identity: string;
  readonly operation: string;
  readonly family: string;
  readonly method: string;
  readonly route: string;
  readonly signature: string;
  readonly signature_digests: readonly string[];
  readonly required_feature: string | null;
  readonly required_features: readonly string[];
  readonly feature_semantics: string;
}

export interface CatalogFamily {
  readonly family: string;
  readonly methods: readonly string[];
  readonly operations: readonly CatalogOperation[];
  readonly required_feature: string | null;
}

export interface CatalogDocument {
  readonly schema_version: number;
  readonly extension_protocol_version: number;
  readonly total_items: number;
  readonly routes: readonly CatalogRoute[];
  readonly families: readonly {
    readonly family: string;
    readonly methods: readonly string[];
    readonly operations: readonly string[];
    readonly required_feature?: string | null;
  }[];
}

export class FacadeCatalog {
  readonly document: CatalogDocument;
  readonly contractDigest: string;
  readonly sourceContractDigest: string;
  private readonly operationRoutes: ReadonlyMap<string, CatalogRoute>;
  private readonly familyRoutes: ReadonlyMap<string, CatalogRoute>;
  private readonly operationList: readonly CatalogOperation[];
  private readonly familyList: readonly CatalogFamily[];

  constructor(path?: string) {
    const resolved = path ?? fileURLToPath(new URL("../../shared/facade-operation-catalog.json", import.meta.url));
    this.document = JSON.parse(readFileSync(resolved, "utf8")) as CatalogDocument;
    const digestPath = path
      ? join(dirname(resolved), "contract-digests.json")
      : fileURLToPath(new URL("../../shared/contract-digests.json", import.meta.url));
    const digests = JSON.parse(readFileSync(digestPath, "utf8")) as {
      contract_digest: string;
      source_contract_digest: string;
    };
    this.contractDigest = digests.contract_digest;
    this.sourceContractDigest = digests.source_contract_digest;
    this.operationRoutes = new Map(
      this.document.routes
        .filter((route) => route.operation !== null)
        .map((route) => [route.operation as string, route]),
    );
    this.familyRoutes = new Map(
      this.document.routes
        .filter((route) => route.operation === null)
        .filter((route) => route.method !== null)
        .map((route) => [route.method as string, route]),
    );

    const operations = this.document.routes.flatMap((route) => {
      const direct = route.operation && route.method
        ? [this.toCatalogOperation(route, route.operation, route.signature_digests ?? [])]
        : [];
      const family = route.method
        ? (route.operation_signatures ?? []).map((entry) =>
            this.toCatalogOperation(route, entry.operation, entry.signature_digests),
          )
        : [];
      return [...direct, ...family];
    });
    operations.sort((left, right) => left.identity.localeCompare(right.identity));
    this.operationList = operations;

    const operationsByFamily = new Map<string, CatalogOperation[]>();
    for (const operation of operations) {
      const family = operationsByFamily.get(operation.family) ?? [];
      family.push(operation);
      operationsByFamily.set(operation.family, family);
    }
    this.familyList = this.document.families
      .map((family) => ({
        family: family.family,
        methods: [...new Set([
          ...family.methods,
          ...(operationsByFamily.get(family.family) ?? []).map((operation) => operation.method),
        ])].sort(),
        operations: operationsByFamily.get(family.family) ?? [],
        required_feature: family.required_feature ?? null,
      }))
      .sort((left, right) => left.family.localeCompare(right.family));
  }

  operations(): readonly CatalogOperation[] {
    return this.operationList;
  }

  families(): readonly CatalogFamily[] {
    return this.familyList;
  }

  signature(operation: string): string {
    const digests = this.operationRoutes.get(operation)?.signature_digests;
    const digest = digests?.[0];
    if (!digest) throw new Error(`operation is absent from the canonical catalog: ${operation}`);
    return digest;
  }

  familySignature(method: string, operation: string): string {
    const route = this.familyRoutes.get(method);
    const digest = route?.operation_signatures
      ?.find((entry) => entry.operation === operation)
      ?.signature_digests[0];
    if (!digest) throw new Error(`family operation is absent from the canonical catalog: ${method}/${operation}`);
    return digest;
  }

  resolve(operation: string): ResolvedOperation {
    const direct = this.operationRoutes.get(operation);
    if (direct?.method && direct.signature_digests?.[0]) {
      return {
        operation,
        family: direct.family,
        method: direct.method,
        signature: direct.signature_digests[0],
      };
    }
    for (const route of this.familyRoutes.values()) {
      const entry = route.operation_signatures?.find((item) => item.operation === operation);
      if (route.method && entry?.signature_digests[0]) {
        return {
          operation,
          family: route.family,
          method: route.method,
          signature: entry.signature_digests[0],
        };
      }
    }
    throw new Error(`operation is absent from the canonical catalog: ${operation}`);
  }

  route(operation: string): CatalogRoute {
    const route = this.operationRoutes.get(operation);
    if (!route) throw new Error(`operation is absent from the canonical catalog: ${operation}`);
    return route;
  }

  private toCatalogOperation(
    route: CatalogRoute,
    operation: string,
    signatureDigests: readonly string[],
  ): CatalogOperation {
    const method = route.method;
    const signature = signatureDigests[0];
    if (!method || !signature) {
      throw new Error(`canonical catalog operation is missing method or signature: ${operation}`);
    }
    return {
      identity: `${route.family}:${operation}`,
      operation,
      family: route.family,
      method,
      route: route.route,
      signature,
      signature_digests: [...signatureDigests],
      required_feature: route.required_feature ?? null,
      required_features: [...(route.required_features ?? [])],
      feature_semantics: route.feature_semantics ?? "default",
    };
  }
}
