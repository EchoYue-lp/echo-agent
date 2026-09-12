package com.echoagent.sdk;

/** HTTP API protocols used by supported LLM endpoints. */
public enum LlmApiProtocol {
    CHAT_COMPLETIONS("chat_completions", "chat/completions"),
    RESPONSES("responses", "responses"),
    ANTHROPIC("anthropic", "messages");

    private final String wireName;
    private final String endpointPath;

    LlmApiProtocol(String wireName, String endpointPath) {
        this.wireName = wireName;
        this.endpointPath = endpointPath;
    }

    public String asStr() { return wireName; }
    public String endpointPath() { return endpointPath; }

    public static LlmApiProtocol tryFromEndpoint(String endpoint) {
        String base = endpoint.split("[?#]", 2)[0].replaceFirst("/+$", "");
        if (base.endsWith("/responses")) return RESPONSES;
        if (base.endsWith("/messages")) return ANTHROPIC;
        if (base.endsWith("/chat/completions")) return CHAT_COMPLETIONS;
        return null;
    }

    public static LlmApiProtocol fromEndpoint(String endpoint) {
        LlmApiProtocol detected = tryFromEndpoint(endpoint);
        if (detected != null) return detected;
        String base = endpoint.split("[?#]", 2)[0];
        return base.contains("anthropic.com/") ? ANTHROPIC : CHAT_COMPLETIONS;
    }
}
