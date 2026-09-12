package com.echoagent.sdk;

/** Provider capability snapshot projected without network or model execution. */
public record ProviderCapabilities(boolean streamingToolCalls, boolean namedSseEvents,
        boolean reasoningContent, boolean imageInput, boolean systemAsTopLevel,
        boolean ndjsonStreaming, boolean toolSupport, boolean structuredOutput,
        boolean requiresVersionHeader, boolean supportsParallelToolCalls,
        boolean supportsToolChoiceNone, String tokenizerName) {
    public static ProviderCapabilities openaiCompatible() {
        return new ProviderCapabilities(true, false, true, true, false, false, true, true, false, true, true, null);
    }
    public static ProviderCapabilities anthropic() {
        return new ProviderCapabilities(false, true, false, true, true, false, true, false, true, true, false, "claude");
    }
    public static ProviderCapabilities ollama() {
        return new ProviderCapabilities(false, false, false, false, false, true, true, false, false, false, false, null);
    }
    public static ProviderCapabilities fromProviderName(String name) {
        String normalized = name == null ? "" : name.toLowerCase(java.util.Locale.ROOT);
        if (normalized.equals("anthropic")) return anthropic();
        if (normalized.equals("ollama")) return ollama();
        return openaiCompatible();
    }
}
