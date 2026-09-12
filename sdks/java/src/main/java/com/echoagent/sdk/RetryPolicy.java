package com.echoagent.sdk;

import java.util.concurrent.ThreadLocalRandom;

/** Retry policy configuration and backoff projection without executing retries. */
public final class RetryPolicy {
    private final int maxRetries;
    private final long baseDelayMs;
    private final long maxDelayMs;
    private final boolean jitter;

    private RetryPolicy(int maxRetries, long baseDelayMs, long maxDelayMs, boolean jitter) {
        this.maxRetries = maxRetries; this.baseDelayMs = baseDelayMs;
        this.maxDelayMs = maxDelayMs; this.jitter = jitter;
    }
    public static RetryPolicy newPolicy(int maxRetries, long baseDelayMs) {
        if (maxRetries < 0 || baseDelayMs < 0) throw new IllegalArgumentException("retry policy values must be non-negative");
        return new RetryPolicy(maxRetries, baseDelayMs, 60_000, false);
    }
    public static RetryPolicy defaults() { return new RetryPolicy(3, 500, 30_000, true); }
    public static RetryPolicy noRetry() { return new RetryPolicy(0, 0, 0, false); }
    public RetryPolicy maxDelay(long delayMs) {
        if (delayMs < 0) throw new IllegalArgumentException("retry max delay must be non-negative");
        return new RetryPolicy(maxRetries, baseDelayMs, delayMs, jitter);
    }
    public RetryPolicy jitter(boolean enabled) { return new RetryPolicy(maxRetries, baseDelayMs, maxDelayMs, enabled); }
    public long delayFor(int attempt) {
        if (attempt < 0) throw new IllegalArgumentException("retry attempt must be non-negative");
        if (attempt == 0) return 0;
        long multiplier = 1L << Math.min(attempt - 1, 10);
        long delay = baseDelayMs > Long.MAX_VALUE / multiplier ? Long.MAX_VALUE : baseDelayMs * multiplier;
        long capped = Math.min(delay, maxDelayMs);
        return jitter && capped > 0 ? ThreadLocalRandom.current().nextLong(capped + 1) : capped;
    }
}
