/** Observed Subagent isolation value without owning isolation execution. */
export class ObservedIsolation {
  public readonly value: string;

  private constructor(value: string) {
    this.value = value;
    Object.freeze(this);
  }

  public static new(value: string): ObservedIsolation {
    if (typeof value !== "string") throw new TypeError("observed isolation must be text");
    const trimmed = value.trim();
    return new ObservedIsolation(trimmed ? Array.from(trimmed).slice(0, 512).join("") : "unknown");
  }

  public static default(): ObservedIsolation {
    return new ObservedIsolation("unknown");
  }

  public asStr(): string {
    return this.value;
  }
}
