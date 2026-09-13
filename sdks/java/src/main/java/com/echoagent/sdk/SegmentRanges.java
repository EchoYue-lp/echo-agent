package com.echoagent.sdk;

/** Message-cache segment indexes without owning cache state. */
public record SegmentRanges(
        SegmentRange system,
        SegmentRange canonical,
        SegmentRange history,
        SegmentRange runtimeContext) {
    public SegmentRanges {
        system = system == null ? new SegmentRange() : system;
        canonical = canonical == null ? new SegmentRange() : canonical;
        history = history == null ? new SegmentRange() : history;
        runtimeContext = runtimeContext == null ? new SegmentRange() : runtimeContext;
    }

    public SegmentRanges() {
        this(new SegmentRange(), new SegmentRange(), new SegmentRange(), new SegmentRange());
    }
}
