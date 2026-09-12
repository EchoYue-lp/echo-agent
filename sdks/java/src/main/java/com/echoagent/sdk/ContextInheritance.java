package com.echoagent.sdk;

import java.util.List;
import java.util.Map;

/** Subagent context inheritance values projected without owning context state. */
public record ContextInheritance(
        List<String> inheritTools,
        Long inheritHistory,
        boolean inheritMemory,
        Map<String, String> injectMetadata) {
    public ContextInheritance {
        if (inheritHistory != null && inheritHistory < 0) {
            throw new IllegalArgumentException("inherit history must be non-negative");
        }
        inheritTools = inheritTools == null ? null : List.copyOf(inheritTools);
        injectMetadata = Map.copyOf(injectMetadata);
    }

    public static ContextInheritance syncDefault() { return new ContextInheritance(null, null, false, Map.of()); }
    public static ContextInheritance freshDefault() { return syncDefault(); }
    public static ContextInheritance forkDefault() { return new ContextInheritance(null, 2L, true, Map.of()); }
    public static ContextInheritance teammateDefault() { return new ContextInheritance(List.of(), 2L, false, Map.of()); }

    public static ContextInheritance forMode(String mode) {
        return switch (mode) {
            case "sync" -> syncDefault();
            case "fork" -> forkDefault();
            case "teammate", "team" -> teammateDefault();
            default -> throw new IllegalArgumentException("unknown execution mode: " + mode);
        };
    }
}
