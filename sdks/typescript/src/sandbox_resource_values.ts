export type ResourceLimitsValue = Readonly<{
  cpuTimeSecs?: bigint;
  memoryBytes?: bigint;
  maxOutputBytes?: bigint;
  maxProcesses?: number;
  network: boolean;
  readOnlyPaths: readonly string[];
  writablePaths: readonly string[];
}>;

function freeze(value: ResourceLimitsValue): ResourceLimitsValue {
  return Object.freeze({
    ...value,
    readOnlyPaths: Object.freeze([...value.readOnlyPaths]),
    writablePaths: Object.freeze([...value.writablePaths]),
  });
}

/** Sandbox resource policy value; process execution remains Rust-owned. */
export const ResourceLimits = Object.freeze({
  default(): ResourceLimitsValue {
    return freeze({ cpuTimeSecs: 30n, memoryBytes: 256n * 1024n * 1024n, maxOutputBytes: 1n * 1024n * 1024n, maxProcesses: 64, network: false, readOnlyPaths: [], writablePaths: [] });
  },
  strict(): ResourceLimitsValue {
    return freeze({ cpuTimeSecs: 10n, memoryBytes: 64n * 1024n * 1024n, maxOutputBytes: 256n * 1024n, maxProcesses: 8, network: false, readOnlyPaths: [], writablePaths: [] });
  },
  unrestricted(): ResourceLimitsValue {
    return freeze({ network: true, readOnlyPaths: [], writablePaths: [] });
  },
});
