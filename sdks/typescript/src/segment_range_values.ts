/** Message-cache segment range values without owning cache state. */
export class SegmentRange {
  public constructor(public readonly start: bigint = 0n, public readonly end: bigint = 0n) {
    if (start < 0n || end < 0n) throw new RangeError("segment range bounds must be non-negative");
    Object.freeze(this);
  }

  public len(): bigint {
    return this.end > this.start ? this.end - this.start : 0n;
  }

  public isEmpty(): boolean {
    return this.len() === 0n;
  }
}
