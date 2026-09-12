package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.List;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.Flow;
import java.util.concurrent.atomic.AtomicBoolean;

/** ACP Session plus its negotiated echo-agent handle. */
public final class SessionHandle implements AutoCloseable {
    private final EchoAgentClient client;
    private final WireHandle agent;
    private final WireHandle wire;
    private final String acpSessionId;
    private final WireHandle taskRun;
    private final AtomicBoolean closed = new AtomicBoolean();

    SessionHandle(EchoAgentClient client, WireHandle agent, WireHandle wire, String acpSessionId, WireHandle taskRun) {
        this.client = client;
        this.agent = agent;
        this.wire = wire;
        this.acpSessionId = acpSessionId;
        this.taskRun = taskRun;
    }

    public WireHandle wire() { return wire; }
    public String acpSessionId() { return acpSessionId; }
    public WireHandle taskRun() { return taskRun; }
    public Flow.Publisher<JsonNode> updates() { return client.updates(acpSessionId); }

    public CompletionStage<JsonNode> prompt(String text) {
        if (closed.get()) return failedClosed("session/prompt");
        var params = JsonSupport.MAPPER.createObjectNode();
        params.put("sessionId", acpSessionId);
        params.set("prompt", JsonSupport.MAPPER.createArrayNode().add(JsonSupport.MAPPER.createObjectNode().put("type", "text").put("text", text)));
        return client.request("session/prompt", params);
    }

    public CompletionStage<RunHandle> startRun(String text, boolean execute) {
        if (closed.get()) return failedClosed("_echo_agent/run/start");
        var params = JsonSupport.MAPPER.createObjectNode();
        params.set("session", wire.toJson());
        var input = params.putObject("input");
        input.put("kind", execute ? "execute" : "chat");
        input.put(execute ? "task" : "text", text);
        return client.request("_echo_agent/run/start", params).thenApply(result -> {
            WireHandle run = WireHandle.fromJson(result.path("run"));
            WireHandle stream = WireHandle.fromJson(result.path("stream"));
            RunHandle handle = new RunHandle(client, run, stream);
            client.publishFirstEvent(stream, result.path("first_event"));
            return handle;
        });
    }

    public CompletionStage<RunHandle> startChat(String text) { return startRun(text, false); }
    public CompletionStage<RunHandle> startExecute(String task) { return startRun(task, true); }

    public CompletionStage<JsonNode> invoke(String operation, List<?> arguments) {
        if (closed.get()) return failedClosed("_echo_agent/facade/invoke");
        var resolved = client.catalog().resolve(operation);
        return "_echo_agent/facade/invoke".equals(resolved.method())
                ? client.call(operation, agent, prepend(arguments))
                : client.call(operation, wire, arguments);
    }

    private List<Object> prepend(List<?> arguments) {
        var values = new java.util.ArrayList<Object>();
        values.add(wire);
        if (arguments != null) values.addAll(arguments);
        return values;
    }

    public CompletionStage<Void> cancel() {
        if (closed.get()) return failedClosed("session/cancel");
        var params = JsonSupport.MAPPER.createObjectNode().put("sessionId", acpSessionId);
        return client.request("session/cancel", params).thenApply(ignored -> null);
    }

    public CompletionStage<Void> closeAsync() {
        if (!closed.compareAndSet(false, true)) return CompletableFuture.completedFuture(null);
        return client.request("_echo_agent/session/close",
                        JsonSupport.MAPPER.createObjectNode().set("session", wire.toJson()))
                .handle((ignored, error) -> {
                    client.closeUpdates(acpSessionId);
                    if (error != null) throw new CompletionException(error);
                    return null;
                });
    }

    @Override public void close() {
        closeAsync().toCompletableFuture().join();
    }

    private <T> CompletionStage<T> failedClosed(String operation) {
        return CompletableFuture.failedFuture(new EchoAgentException(
                "stale_handle", "Session handle is closed", "never", operation, wire.toJson()));
    }
}
