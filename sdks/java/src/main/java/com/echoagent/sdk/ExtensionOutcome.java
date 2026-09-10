package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

/** Exactly-one-settlement value returned by a typed extension callback. */
public interface ExtensionOutcome {
    JsonNode toJson();

    static ExtensionOutcome stream(WireHandle stream) {
        return JsonExtensionOutcome.stream(stream);
    }

    static ExtensionOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }

    static ExtensionOutcome error(String code, String message, String retryable, JsonNode details) {
        return JsonExtensionOutcome.error(code, message, retryable, details);
    }
}
