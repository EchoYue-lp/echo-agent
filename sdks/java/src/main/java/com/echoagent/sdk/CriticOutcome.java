package com.echoagent.sdk;

/** Operation-checked outcomes for Critic callbacks. */
public interface CriticOutcome extends ExtensionOutcome {
    static CriticOutcome result(Critique value) {
        if (value == null) throw new IllegalArgumentException("critique is required");
        return JsonExtensionOutcome.result("critic_critique", value.toJson());
    }

    static CriticOutcome stream(WireHandle stream) { return JsonExtensionOutcome.stream(stream); }

    static CriticOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }
}
