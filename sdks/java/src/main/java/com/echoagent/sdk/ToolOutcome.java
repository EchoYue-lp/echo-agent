package com.echoagent.sdk;

/** Operation-checked outcomes for Tool callbacks. */
public interface ToolOutcome extends ExtensionOutcome {
    static ToolOutcome result(ToolResult value) {
        return JsonExtensionOutcome.result("tool_execute", value.toJson());
    }

    static ToolOutcome validated() {
        return JsonExtensionOutcome.result("tool_validate_parameters", com.fasterxml.jackson.databind.node.NullNode.instance);
    }

    static ToolOutcome invalidParameters(String message) {
        if (message == null) return validated();
        return JsonExtensionOutcome.result("tool_validate_parameters",
                com.fasterxml.jackson.databind.node.TextNode.valueOf(message));
    }

    static ToolOutcome stream(WireHandle stream) { return JsonExtensionOutcome.stream(stream); }

    static ToolOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }
}
