package com.echoagent.sdk;

import java.math.BigInteger;
import java.util.List;

/** Sandbox resource policy value; process execution remains Rust-owned. */
public record ResourceLimits(BigInteger cpuTimeSecs, BigInteger memoryBytes,
        BigInteger maxOutputBytes, Integer maxProcesses, boolean network,
        List<String> readOnlyPaths, List<String> writablePaths) {
    public ResourceLimits {
        readOnlyPaths = List.copyOf(readOnlyPaths == null ? List.of() : readOnlyPaths);
        writablePaths = List.copyOf(writablePaths == null ? List.of() : writablePaths);
    }

    public static ResourceLimits defaults() {
        return new ResourceLimits(BigInteger.valueOf(30), BigInteger.valueOf(256).multiply(BigInteger.ONE.shiftLeft(20)),
                BigInteger.ONE.shiftLeft(20), 64, false, List.of(), List.of());
    }

    public static ResourceLimits strict() {
        return new ResourceLimits(BigInteger.TEN, BigInteger.valueOf(64).multiply(BigInteger.ONE.shiftLeft(20)),
                BigInteger.valueOf(256).multiply(BigInteger.ONE.shiftLeft(10)), 8, false, List.of(), List.of());
    }

    public static ResourceLimits unrestricted() {
        return new ResourceLimits(null, null, null, null, true, List.of(), List.of());
    }
}
