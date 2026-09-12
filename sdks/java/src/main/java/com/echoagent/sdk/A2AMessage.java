package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.ArrayList;
import java.util.List;

/** A2A message value with text and file parts. */
public final class A2AMessage {
    private final String role;
    private final List<JsonNode> parts;

    private A2AMessage(String role, List<JsonNode> parts) {
        this.role = role;
        if (parts == null) {
            this.parts = List.of();
        } else {
            var copied = new ArrayList<JsonNode>();
            for (JsonNode node : parts) copied.add(node.deepCopy());
            this.parts = List.copyOf(copied);
        }
    }

    public static A2AMessage userText(String text) { return text("user", text); }
    public static A2AMessage agentText(String text) { return text("agent", text); }

    public String role() { return role; }
    public List<JsonNode> parts() {
        var copied = new ArrayList<JsonNode>();
        for (JsonNode node : parts) copied.add(node.deepCopy());
        return List.copyOf(copied);
    }

    public String textContent() {
        return parts.stream()
                .filter(part -> "text".equals(part.path("type").asText()) && part.path("text").isTextual())
                .map(part -> part.path("text").textValue())
                .reduce((left, right) -> left + "\n" + right)
                .orElse("");
    }

    public ObjectNode toJson() {
        var result = JsonSupport.MAPPER.createObjectNode().put("role", role);
        ArrayNode values = result.putArray("parts");
        parts.forEach(values::add);
        return result;
    }

    private static A2AMessage text(String role, String text) {
        if (text == null) throw new IllegalArgumentException("message text must not be null");
        var part = JsonSupport.MAPPER.createObjectNode().put("type", "text").put("text", text);
        return new A2AMessage(role, List.of(part));
    }
}
