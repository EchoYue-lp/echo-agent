package com.echoagent.sdk;

import java.util.Set;

/** Consumer-provided model policy overrides. */
public record ModelProfileOverride(Boolean supportsParallelToolCalls,
        Boolean supportsToolChoiceNone, Boolean supportsStructuredOutput,
        Integer contextWindow, Set<String> excludedTools, String promptSuffix) {
    public ModelProfileOverride() {
        this(null, null, null, null, Set.of(), null);
    }

    public ModelProfileOverride {
        excludedTools = Set.copyOf(excludedTools == null ? Set.of() : excludedTools);
    }
}
