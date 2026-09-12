package com.echoagent.sdk;

import java.util.Map;
import java.util.Objects;
import java.util.UUID;

/** Immutable transport identity projection without owning a live stream. */
public final class EventIdentity {
    private final StreamId streamId;
    private final String conversationId;
    private final String runId;
    private final String turnId;
    private final String messageId;
    private final String executionId;
    private final String parentEventId;

    private EventIdentity(StreamId streamId, String conversationId, String runId, String turnId,
            String messageId, String executionId, String parentEventId) {
        this.streamId = Objects.requireNonNull(streamId, "streamId");
        this.conversationId = optional(conversationId, "conversation_id");
        this.runId = optional(runId, "run_id");
        this.turnId = requireNonBlank(turnId, "turn_id");
        this.messageId = optional(messageId, "message_id");
        this.executionId = optional(executionId, "execution_id");
        this.parentEventId = optional(parentEventId, "event_id");
    }

    public static EventIdentity newIdentity(String streamId, String turnId) {
        return new EventIdentity(StreamId.newId(streamId), null, null, turnId, null, null, null);
    }

    public static EventIdentity forRun(String runId) {
        runId = requireNonBlank(runId, "run_id");
        return new EventIdentity(StreamId.newId(UUID.randomUUID().toString()), null, runId,
                runId, null, runId, null);
    }

    public static EventIdentity forChat(String conversationId, String turnId, String messageId, String runId) {
        return new EventIdentity(StreamId.newId(UUID.randomUUID().toString()), conversationId, runId,
                turnId, requireNonBlank(messageId, "message_id"), null, null);
    }

    public static EventIdentity fromInvocation(Map<String, ?> invocation) {
        Object runtime = invocation == null ? null : invocation.get("runtime");
        return fromRuntimeContext(runtime instanceof Map<?, ?> map ? map : null);
    }

    public static EventIdentity fromRuntimeContext(Map<?, ?> runtime) {
        String runId = stringValue(runtime, "run_id");
        String executionId = stringValue(runtime, "execution_id");
        String turnId = firstNonBlank(stringValue(runtime, "turn_id"), executionId, runId,
                UUID.randomUUID().toString());
        String parent = stringValue(runtime, "parent_event_id");
        return new EventIdentity(StreamId.newId(UUID.randomUUID().toString()),
                stringValue(runtime, "conversation_id"), runId, turnId,
                stringValue(runtime, "message_id"), executionId, parent);
    }

    public void validate() {
        StreamId.newId(streamId.asStr());
        requireNonBlank(turnId, "turn_id");
    }

    public StreamId streamId() { return streamId; }
    public String conversationId() { return conversationId; }
    public String runId() { return runId; }
    public String turnId() { return turnId; }
    public String messageId() { return messageId; }
    public String executionId() { return executionId; }
    public String parentEventId() { return parentEventId; }

    public EventIdentity withConversationId(String value) {
        return copy(requireNonBlank(value, "conversation_id"), runId, messageId, executionId, parentEventId);
    }
    public EventIdentity withRunId(String value) {
        return copy(conversationId, requireNonBlank(value, "run_id"), messageId, executionId, parentEventId);
    }
    public EventIdentity withMessageId(String value) {
        return copy(conversationId, runId, requireNonBlank(value, "message_id"), executionId, parentEventId);
    }
    public EventIdentity withExecutionId(String value) {
        return copy(conversationId, runId, messageId, requireNonBlank(value, "execution_id"), parentEventId);
    }
    public EventIdentity withParentEventId(String value) {
        return copy(conversationId, runId, messageId, executionId, requireNonBlank(value, "event_id"));
    }

    private EventIdentity copy(String conversation, String run, String message, String execution, String parent) {
        return new EventIdentity(streamId, conversation, run, turnId, message, execution, parent);
    }

    private static String stringValue(Map<?, ?> values, String key) {
        if (values == null) return null;
        Object value = values.get(key);
        return value == null ? null : String.valueOf(value);
    }

    private static String firstNonBlank(String... values) {
        for (String value : values) if (value != null && !value.isBlank()) return value;
        return "";
    }

    private static String optional(String value, String label) {
        return value == null ? null : requireNonBlank(value, label);
    }

    private static String requireNonBlank(String value, String label) {
        Objects.requireNonNull(value, label);
        if (value.isBlank()) throw new IllegalArgumentException(label + " must not be empty");
        return value;
    }
}
