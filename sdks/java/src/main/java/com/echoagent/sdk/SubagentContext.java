package com.echoagent.sdk;

import java.util.List;

/** Subagent context snapshot values without owning tools, messages, or stores. */
public record SubagentContext(
        List<Object> toolDefinitions,
        List<Object> messages,
        boolean storePresent,
        String parentGoal,
        List<String> allowedTools) {
    public SubagentContext {
        toolDefinitions = List.copyOf(toolDefinitions);
        messages = List.copyOf(messages);
        allowedTools = allowedTools == null ? null : List.copyOf(allowedTools);
    }

    public SubagentContext() { this(List.of(), List.of(), false, null, null); }

    public static SubagentContext empty() { return new SubagentContext(); }

    public boolean hasContent() {
        return !toolDefinitions.isEmpty() || !messages.isEmpty() || storePresent
                || parentGoal != null || allowedTools != null;
    }
}
