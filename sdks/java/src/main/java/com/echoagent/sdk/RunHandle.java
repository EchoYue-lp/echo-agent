package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.math.BigInteger;
import java.time.Duration;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.Flow;
import java.util.concurrent.atomic.AtomicBoolean;

/** Finite run handle with a Flow.Publisher event view. */
public final class RunHandle implements AutoCloseable {
    private final EchoAgentClient client;
    private final WireHandle wire;
    private final WireHandle stream;
    private final AtomicBoolean closed = new AtomicBoolean();

    RunHandle(EchoAgentClient client, WireHandle wire, WireHandle stream) {
        this.client = client;
        this.wire = wire;
        this.stream = stream;
    }

    public WireHandle wire() { return wire; }
    public WireHandle stream() { return stream; }
    public Flow.Publisher<JsonNode> events() { return client.events(stream); }
    public CompletionStage<JsonNode> get() {
        if (closed.get()) return failedClosed("_echo_agent/run/get");
        return client.request("_echo_agent/run/get", requestParams());
    }
    /** Reads the settled TurnReceipt status through the Host-owned Run receiver. */
    public CompletionStage<JsonNode> status() {
        return receiptOperation("echo_orchestration::runtime::turn_driver::TurnReceipt::status");
    }
    /** Reads the settled TurnOutcome status through the Host-owned Run receiver. */
    public CompletionStage<JsonNode> outcomeStatus() {
        return receiptOperation("echo_orchestration::runtime::turn_driver::TurnOutcome::status");
    }
    /** Reads settled execution usage, preserving all WireU64 fields as text nodes. */
    public CompletionStage<JsonNode> usage() {
        return receiptOperation("echo_orchestration::runtime::turn_driver::TurnReceipt::usage");
    }
    public CompletionStage<JsonNode> waitForSettlement() {
        return waitForSettlement(null);
    }
    public CompletionStage<JsonNode> waitForSettlement(Duration timeout) {
        if (closed.get()) return failedClosed("_echo_agent/run/wait");
        return waitInternal(timeout);
    }

    private CompletionStage<JsonNode> waitInternal(Duration timeout) {
        var params = requestParams();
        if (timeout != null) {
            if (timeout.isNegative()) return CompletableFuture.failedFuture(new EchoAgentException(
                    "invalid_value", "run wait timeout must not be negative", "never",
                    "_echo_agent/run/wait", null));
            var wireTimeout = JsonSupport.MAPPER.createObjectNode()
                    .put("seconds", Long.toString(timeout.toSeconds()))
                    .put("nanos", timeout.getNano());
            params.set("timeout", wireTimeout);
        }
        return client.request("_echo_agent/run/wait", params);
    }
    public CompletionStage<JsonNode> cancel() {
        if (closed.get()) return failedClosed("_echo_agent/run/cancel");
        return cancelInternal();
    }
    public CompletionStage<JsonNode> steer(String text) {
        if (closed.get()) return failedClosed("_echo_agent/run/steer");
        var params = JsonSupport.MAPPER.createObjectNode();
        params.set("run", wire.toJson());
        params.put("text", text);
        return client.request("_echo_agent/run/steer", params);
    }
    public CompletionStage<JsonNode> replay(String afterSequence, int maxEvents) {
        if (closed.get()) return failedClosed("_echo_agent/run/replay");
        var params = JsonSupport.MAPPER.createObjectNode();
        params.set("stream", stream.toJson());
        params.put("after_sequence", afterSequence == null ? "0" : afterSequence);
        params.put("max_events", Integer.toString(maxEvents));
        return client.request("_echo_agent/run/replay", params);
    }
    /**
     * Requests cancellation, waits for the bounded Host settlement, and then
     * closes the event publisher. A timeout is surfaced as a typed failure;
     * the local publisher is still released in all cases.
     */
    public CompletionStage<Void> closeAsync() {
        if (!closed.compareAndSet(false, true)) return CompletableFuture.completedFuture(null);
        return cancelInternal()
                .thenCompose(ignored -> waitInternal(Duration.ofSeconds(5)))
                .handle((wait, error) -> {
                    client.closeEvents(stream);
                    if (error != null) throw new CompletionException(error);
                    if (wait == null || !wait.path("settled").asBoolean(false)) {
                        throw new CompletionException(new EchoAgentException(
                                "cancellation_timeout", "run did not settle before close timeout",
                                "after_delay", "_echo_agent/run/wait", wire.toJson()));
                    }
                    return null;
                });
    }

    @Override public void close() {
        closeAsync().toCompletableFuture().join();
    }

    private CompletionStage<JsonNode> cancelInternal() {
        return client.request("_echo_agent/run/cancel", requestParams());
    }

    private CompletionStage<JsonNode> receiptOperation(String operation) {
        if (closed.get()) return failedClosed(operation);
        return client.call(operation, wire, java.util.List.of());
    }

    private com.fasterxml.jackson.databind.node.ObjectNode requestParams() {
        return JsonSupport.MAPPER.createObjectNode().set("run", wire.toJson());
    }

    private <T> CompletionStage<T> failedClosed(String operation) {
        return CompletableFuture.failedFuture(new EchoAgentException(
                "stale_handle", "Run handle is closed", "never", operation, wire.toJson()));
    }
}
