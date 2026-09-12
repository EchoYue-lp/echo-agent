package com.echoagent.sdk;

/** Operation-checked outcomes for ContextCompressor callbacks. */
public interface CompressionOutcome extends ExtensionOutcome {
    static CompressionOutcome result(CompressionOutput value) {
        if (value == null) throw new IllegalArgumentException("compression output is required");
        return JsonExtensionOutcome.result("compressor_compress", value.toJson());
    }

    static CompressionOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }
}
