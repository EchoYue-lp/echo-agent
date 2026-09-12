import type { ToolResultData } from "./types.js";
import { ToolResultValue } from "./helpers.js";

/** Pagination metadata projection without owning collection state. */
export class PageInfo {
  public constructor(
    public readonly nextCursor: string | null = null,
    public readonly truncated = false,
    public readonly totalKnown = false,
    public readonly total: bigint | null = null,
    public readonly returned: bigint = 0n,
  ) {}

  public applyTo(result: ToolResultValue): ToolResultValue {
    let updated = result
      .withTruncated(result.truncated || this.truncated)
      .withMeta("page.truncated", String(this.truncated))
      .withMeta("page.total_known", String(this.totalKnown))
      .withMeta("page.returned", this.returned.toString());
    if (this.total !== null) updated = updated.withMeta("page.total", this.total.toString());
    if (this.nextCursor !== null) {
      updated = updated.withMeta("page.next_cursor", this.nextCursor);
      const continuation = JSON.stringify({
        next_cursor: this.nextCursor,
        returned: this.returned.toString(),
        total: this.total === null ? null : this.total.toString(),
        total_known: this.totalKnown,
        truncated: true,
      });
      updated = updated.withOutput(`${updated.output}\n[page]${continuation}`);
    }
    return updated;
  }
}
