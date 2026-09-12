package com.echoagent.sdk;

import java.math.BigInteger;

/** Product-neutral execution usage value projected without owning a run. */
public record ExecutionUsage(BigInteger durationMs, BigInteger tokensUsed, BigInteger iterations) {
    public BigInteger durationMillis() { return durationMs == null ? BigInteger.ZERO : durationMs; }
}
