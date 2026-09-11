package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.ArrayList;
import java.util.List;
import java.math.BigInteger;

/** Immutable A2A artifact value. */
public final class A2AArtifact {
    private final String name;
    private final BigInteger index;
    private final List<JsonNode> parts;
    private final boolean append;

    private A2AArtifact(List<JsonNode> parts, String name, BigInteger index, boolean append) {
        if (parts == null || parts.stream().anyMatch(value -> value == null)) {
            throw new IllegalArgumentException("artifact parts must not be null");
        }
        if (index != null && (index.signum() < 0 || index.bitLength() > 64)) {
            throw new IllegalArgumentException("artifact index must fit Rust usize");
        }
        this.name = name;
        this.index = index;
        var copied = new ArrayList<JsonNode>();
        for (JsonNode part : parts) copied.add(copyPart(part));
        this.parts = List.copyOf(copied);
        this.append = append;
    }

    public static A2AArtifact newArtifact(List<JsonNode> parts, String name, BigInteger index, boolean append) {
        return new A2AArtifact(parts, name, index, append);
    }

    public String name() { return name; }
    public BigInteger index() { return index; }
    public List<JsonNode> parts() {
        var copied = new ArrayList<JsonNode>();
        for (JsonNode part : parts) copied.add(copyPart(part));
        return List.copyOf(copied);
    }
    public boolean append() { return append; }

    private static JsonNode copyPart(JsonNode part) {
        if (part == null || !part.isObject() || !part.path("type").isTextual()) {
            throw new IllegalArgumentException("artifact part must be a typed object");
        }
        String type = part.path("type").textValue();
        if ("text".equals(type) && part.path("text").isTextual()) return part.deepCopy();
        if ("file".equals(type) && part.path("mimeType").isTextual() && part.path("data").isTextual()) {
            return part.deepCopy();
        }
        throw new IllegalArgumentException("artifact part must be a valid text or file part");
    }
}
