package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.concurrent.CompletionStage;

/** Host-issued Agent definition handle. */
public final class AgentHandle implements AutoCloseable {
    private final EchoAgentClient client;
    private final WireHandle wire;
    private CompletionStage<Void> closeStage;

    AgentHandle(EchoAgentClient client, WireHandle wire) {
        this.client = client;
        this.wire = wire;
    }

    public WireHandle wire() { return wire; }

    public CompletionStage<JsonNode> describe() {
        return client.request("_echo_agent/agent/describe", JsonSupport.MAPPER.createObjectNode().set("agent", wire.toJson()));
    }

    public CompletionStage<SessionHandle> createSession(String cwd) {
        var params = JsonSupport.MAPPER.createObjectNode();
        params.set("agent", wire.toJson());
        if (cwd == null || cwd.isBlank()) params.putNull("working_dir");
        else params.set("working_dir", JsonSupport.MAPPER.createObjectNode().put("encoding", "utf8").put("path", cwd));
        return client.request("_echo_agent/session/create", params).thenApply(result -> new SessionHandle(
                client,
                wire,
                WireHandle.fromJson(result.path("session")),
                result.path("acp_session_id").asText(),
                WireHandle.fromJson(result.path("task_run"))));
    }

    public CompletionStage<SessionHandle> createSession() { return createSession(null); }

    /** Awaitable close used by asynchronous callers and the synchronous AutoCloseable bridge. */
    public synchronized CompletionStage<Void> closeAsync() {
        if (closeStage != null) return closeStage;
        closeStage = client.request(
                        "_echo_agent/agent/close",
                        JsonSupport.MAPPER.createObjectNode().set("agent", wire.toJson()))
                .thenApply(ignored -> null);
        return closeStage;
    }

    @Override public void close() {
        // AutoCloseable has a synchronous contract; block only at this outer
        // boundary so the close request is delivered before the handle exits.
        closeAsync().toCompletableFuture().join();
    }
}
