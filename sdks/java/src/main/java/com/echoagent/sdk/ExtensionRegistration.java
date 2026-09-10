package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.concurrent.CompletionStage;

/** Connection-owned registration for a host-language implementation. */
public final class ExtensionRegistration implements AutoCloseable {
    private final EchoAgentClient client;
    private final WireHandle wire;
    private boolean closed;

    ExtensionRegistration(EchoAgentClient client, WireHandle wire) {
        this.client = client;
        this.wire = wire;
    }

    public WireHandle wire() { return wire; }

    public CompletionStage<JsonNode> unregister() {
        if (closed) return java.util.concurrent.CompletableFuture.completedFuture(JsonSupport.MAPPER.createObjectNode());
        closed = true;
        return client.unregisterExtension(wire);
    }

    @Override public void close() {
        unregister();
    }
}
