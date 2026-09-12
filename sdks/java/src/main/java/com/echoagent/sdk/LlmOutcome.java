package com.echoagent.sdk;

/** Operation-checked outcomes for LlmClient callbacks. */
public interface LlmOutcome extends ExtensionOutcome {
    static LlmOutcome result(LlmChatResponse value) {
        return JsonExtensionOutcome.result("llm_chat", value.toJson());
    }

    static LlmOutcome stream(WireHandle stream) { return JsonExtensionOutcome.stream(stream); }

    static LlmOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }
}
