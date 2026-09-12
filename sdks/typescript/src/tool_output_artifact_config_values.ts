/** Tool output artifact configuration without owning artifact writing. */
export class ToolOutputArtifactConfigValue {
  public readonly rootDir: string;
  public readonly retention: string;
  public readonly thresholdBytes: bigint;
  public readonly maxAgeSecs: bigint | null;

  private constructor(rootDir: string, retention: string, thresholdBytes: bigint, maxAgeSecs: bigint | null) {
    this.rootDir = rootDir;
    this.retention = retention;
    this.thresholdBytes = thresholdBytes;
    this.maxAgeSecs = maxAgeSecs;
    Object.freeze(this);
  }

  public static new(rootDir: string, retention: string): ToolOutputArtifactConfigValue {
    return new ToolOutputArtifactConfigValue(rootDir, retention, 1_048_576n, null);
  }

  public static default(): ToolOutputArtifactConfigValue {
    const temp = process.env.TMPDIR || "/tmp";
    return new ToolOutputArtifactConfigValue(`${temp}/echo_agent_artifacts/tool-logs`, "temporary_1h", 1_048_576n, 3_600n);
  }

  public withThresholdBytes(value: bigint): ToolOutputArtifactConfigValue {
    return new ToolOutputArtifactConfigValue(this.rootDir, this.retention, value > 0n ? value : 1n, this.maxAgeSecs);
  }

  public withMaxAgeSecs(value: bigint | null): ToolOutputArtifactConfigValue {
    return new ToolOutputArtifactConfigValue(this.rootDir, this.retention, this.thresholdBytes, value);
  }
}
