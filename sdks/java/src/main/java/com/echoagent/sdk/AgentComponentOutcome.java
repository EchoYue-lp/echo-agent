package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

/** Operation-checked result for an Agent component callback. */
public interface AgentComponentOutcome extends ExtensionOutcome {
    static AgentComponentOutcome result(String component, AgentComponentResult resultValue) {
        if (resultValue == null) throw new IllegalArgumentException("resultValue must not be null");
        var normalizedComponent = TypedExtensionSupport.requiredText(component, "component", 128);
        if (!resultValue.component().equals(normalizedComponent)) {
            throw new IllegalArgumentException("Agent component result kind does not match its operation");
        }
        var result = JsonSupport.MAPPER.createObjectNode()
                .put("component", normalizedComponent);
        var typed = result.putObject("result")
                .put("operation", resultValue.operation());
        JsonNode value = resultValue.value();
        if (value != null) typed.set("value", value.deepCopy());
        return JsonExtensionOutcome.result("agent_component_call", result);
    }

    static AgentComponentOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }
}
