package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.math.BigInteger;
import java.util.concurrent.CompletionStage;

/** Monotonic writer for SDK-to-Host extension stream notifications. */
public final class ExtensionStreamWriter {
    private final EchoAgentClient client;
    private final WireHandle stream;
    private BigInteger sequence = BigInteger.ZERO;
    private boolean closed;

    public ExtensionStreamWriter(EchoAgentClient client, WireHandle stream) {
        this.client = client;
        this.stream = stream;
    }

    private synchronized String next() {
        if (closed) throw new IllegalStateException("extension stream is already closed");
        if (sequence.equals(JsonSupport.MAX_U64)) {
            throw new IllegalStateException("extension stream sequence exceeds u64");
        }
        sequence = sequence.add(BigInteger.ONE);
        return sequence.toString();
    }

    public CompletionStage<Void> chunk(JsonNode value) {
        return send("chunk", value);
    }

    public CompletionStage<Void> agentComponentChunk(AgentComponentStreamChunk event) {
        if (event == null) throw new IllegalArgumentException("event must not be null");
        var value = JsonSupport.MAPPER.createObjectNode().put("kind", "agent_component");
        value.set("value", event.toJson());
        return chunk(value);
    }

    public CompletionStage<Void> complete(JsonNode value) {
        CompletionStage<Void> result = send("complete", value);
        closed = true;
        return result;
    }

    public CompletionStage<Void> agentComponentComplete(AgentComponentStreamComplete terminal) {
        if (terminal == null) throw new IllegalArgumentException("terminal must not be null");
        var value = JsonSupport.MAPPER.createObjectNode().put("kind", "agent_component");
        value.set("value", terminal.toJson());
        return complete(value);
    }

    public CompletionStage<Void> failed(JsonNode error) {
        CompletionStage<Void> result = send("failed", error);
        closed = true;
        return result;
    }

    public CompletionStage<Void> cancelled() {
        CompletionStage<Void> result = send("cancelled", null);
        closed = true;
        return result;
    }

    private CompletionStage<Void> send(String event, JsonNode value) {
        var params = JsonSupport.MAPPER.createObjectNode();
        params.put("event", event);
        params.set("stream", stream.toJson());
        params.put("sequence", next());
        if ("chunk".equals(event) || "complete".equals(event)) params.set("value", value);
        if ("failed".equals(event)) params.set("error", value);
        return client.notify("_echo_agent/extension/stream", params);
    }
}
