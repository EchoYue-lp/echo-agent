package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.Collections;
import java.util.Map;

/** A2A authentication scheme value. */
public final class AuthenticationScheme {
    private final String scheme;
    private final Map<String, JsonNode> config;

    public AuthenticationScheme(String scheme, Map<String, JsonNode> config) {
        if (scheme == null) throw new IllegalArgumentException("authentication scheme must not be null");
        this.scheme = scheme;
        var copied = new java.util.LinkedHashMap<String, JsonNode>();
        if (config != null) config.forEach((key, value) -> copied.put(key, value == null ? null : value.deepCopy()));
        this.config = Collections.unmodifiableMap(copied);
    }

    public String scheme() { return scheme; }
    public Map<String, JsonNode> config() {
        var copied = new java.util.LinkedHashMap<String, JsonNode>();
        config.forEach((key, value) -> copied.put(key, value == null ? null : value.deepCopy()));
        return Collections.unmodifiableMap(copied);
    }
}
