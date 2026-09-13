package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.ArrayList;
import java.util.Collection;
import java.util.List;

/** Read-only prompt layout projection; cache placement stays in Rust. */
public final class PromptCacheLayout {
    private final List<JsonNode> system;
    private final List<JsonNode> canonical;
    private final List<JsonNode> history;
    private final List<JsonNode> runtimeContext;
    private final List<JsonNode> tools;

    private PromptCacheLayout(
            List<JsonNode> system,
            List<JsonNode> canonical,
            List<JsonNode> history,
            List<JsonNode> runtimeContext,
            List<JsonNode> tools) {
        this.system = List.copyOf(system);
        this.canonical = List.copyOf(canonical);
        this.history = List.copyOf(history);
        this.runtimeContext = List.copyOf(runtimeContext);
        this.tools = List.copyOf(tools);
    }

    public static PromptCacheLayout fromMessages(
            Collection<? extends JsonNode> messages,
            Collection<? extends JsonNode> tools) {
        List<JsonNode> messageValues = snapshots(messages, "messages");
        List<JsonNode> toolValues = snapshots(tools, "tools");
        int systemEnd = messageValues.size();
        for (int index = 0; index < messageValues.size(); index++) {
            if (!"system".equals(messageValues.get(index).path("role").asText())) {
                systemEnd = index;
                break;
            }
        }

        int canonicalStart = systemEnd;
        for (int index = 0; index < systemEnd; index++) {
            String text = messageText(messageValues.get(index));
            if (text != null && text.contains("Canonical context")) {
                canonicalStart = index;
                break;
            }
        }

        int runtimeStart = messageValues.size();
        for (int index = messageValues.size() - 1; index >= 0; index--) {
            String text = messageText(messageValues.get(index));
            if (text != null && text.stripLeading().startsWith("[runtime_context:")) {
                runtimeStart = index;
                while (runtimeStart > systemEnd) {
                    String previous = messageText(messageValues.get(runtimeStart - 1));
                    if (previous == null || !previous.stripLeading().startsWith("[runtime_context:")) break;
                    runtimeStart -= 1;
                }
                break;
            }
        }

        return new PromptCacheLayout(
                messageValues.subList(0, Math.min(canonicalStart, systemEnd)),
                canonicalStart < systemEnd ? messageValues.subList(canonicalStart, systemEnd) : List.of(),
                messageValues.subList(systemEnd, runtimeStart),
                messageValues.subList(runtimeStart, messageValues.size()),
                toolValues);
    }

    public List<JsonNode> system() { return copies(system); }
    public List<JsonNode> canonical() { return copies(canonical); }
    public List<JsonNode> history() { return copies(history); }
    public List<JsonNode> runtimeContext() { return copies(runtimeContext); }
    public List<JsonNode> tools() { return copies(tools); }

    public SegmentRanges segmentRanges() {
        long systemLength = system.size();
        long canonicalLength = canonical.size();
        long historyLength = history.size();
        long runtimeLength = runtimeContext.size();
        long canonicalEnd = systemLength + canonicalLength;
        long historyEnd = canonicalEnd + historyLength;
        return new SegmentRanges(
                new SegmentRange(0L, systemLength),
                new SegmentRange(systemLength, canonicalEnd),
                new SegmentRange(canonicalEnd, historyEnd),
                new SegmentRange(historyEnd, historyEnd + runtimeLength));
    }

    private static List<JsonNode> snapshots(Collection<? extends JsonNode> values, String name) {
        if (values == null) throw new IllegalArgumentException(name + " must not be null");
        var snapshots = new ArrayList<JsonNode>();
        for (JsonNode value : values) {
            if (value == null || !value.isObject()) {
                throw new IllegalArgumentException(name + " entries must be JSON objects");
            }
            snapshots.add(value.deepCopy());
        }
        return List.copyOf(snapshots);
    }

    private static List<JsonNode> copies(List<JsonNode> values) {
        var copies = new ArrayList<JsonNode>();
        for (JsonNode value : values) copies.add(value.deepCopy());
        return List.copyOf(copies);
    }

    private static String messageText(JsonNode message) {
        JsonNode content = message.get("content");
        if (content == null) return null;
        if (content.isTextual()) return content.textValue();
        if (content.isObject()
                && "string".equals(content.path("kind").asText())
                && content.path("value").isTextual()) {
            return content.path("value").textValue();
        }
        return null;
    }
}
