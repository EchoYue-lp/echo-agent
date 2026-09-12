package com.echoagent.sdk;

/** Message-cache segment range values without owning cache state. */
public record SegmentRange(long start, long end) {
    public SegmentRange {
        if (start < 0 || end < 0) throw new IllegalArgumentException("segment range bounds must be non-negative");
    }

    public SegmentRange() { this(0L, 0L); }

    public long len() { return end > start ? end - start : 0L; }

    public boolean isEmpty() { return len() == 0L; }
}
