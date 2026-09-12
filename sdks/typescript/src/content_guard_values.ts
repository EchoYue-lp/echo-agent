/** Content-guard decisions projected without owning guard execution. */
export type ContentGuardResult =
  | { readonly kind: "pass" }
  | { readonly kind: "detected"; readonly piiTypes: readonly string[] }
  | { readonly kind: "rejected"; readonly piiTypes: readonly string[] }
  | { readonly kind: "redacted"; readonly content: string };

export function contentGuardPass(): ContentGuardResult {
  return Object.freeze({ kind: "pass" });
}

export function contentGuardDetected(piiTypes: readonly string[]): ContentGuardResult {
  return Object.freeze({ kind: "detected", piiTypes: Object.freeze([...piiTypes]) });
}

export function contentGuardRejected(piiTypes: readonly string[]): ContentGuardResult {
  return Object.freeze({ kind: "rejected", piiTypes: Object.freeze([...piiTypes]) });
}

export function contentGuardRedacted(content: string): ContentGuardResult {
  if (typeof content !== "string") throw new TypeError("redacted content must be text");
  return Object.freeze({ kind: "redacted", content });
}

export function contentGuardResultIsRejected(result: ContentGuardResult): boolean {
  return result.kind === "rejected";
}
