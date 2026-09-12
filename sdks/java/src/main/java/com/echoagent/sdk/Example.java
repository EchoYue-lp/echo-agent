package com.echoagent.sdk;

import java.nio.file.Path;
import java.util.List;
import java.util.Map;

/** Minimal source-build smoke example; arguments are explicit Host/config paths. */
public final class Example {
    private Example() {}

    private static com.fasterxml.jackson.databind.JsonNode smokeOutcome() {
        var content = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "string")
                .put("value", "java-smoke");
        var message = JsonSupport.MAPPER.createObjectNode();
        message.put("role", "assistant");
        message.set("content", content);
        var raw = JsonSupport.MAPPER.createObjectNode();
        raw.put("kind", "map");
        raw.set("value", JsonSupport.MAPPER.createArrayNode());
        var value = JsonSupport.MAPPER.createObjectNode();
        value.set("message", message);
        value.put("finish_reason", "stop");
        value.set("raw", raw);
        var result = JsonSupport.MAPPER.createObjectNode();
        result.put("operation", "llm_chat");
        result.set("value", value);
        var outcome = JsonSupport.MAPPER.createObjectNode();
        outcome.put("outcome", "result");
        outcome.set("result", result);
        return outcome;
    }

    public static void main(String[] args) throws Exception {
        if (args.length < 3) {
            throw new IllegalArgumentException("usage: Example <host-command> <host-config> <catalog>");
        }
        Path catalog = Path.of(args[2]);
        Path digest = catalog.resolveSibling("contract-digests.json");
        EchoAgentClient client = EchoAgentClient
                .spawn(args[0], List.of("--config", args[1]), catalog, digest, Map.of())
                .toCompletableFuture()
                .get();
        try {
            var descriptor = JsonSupport.MAPPER.createObjectNode();
            descriptor.put("kind", "llm_client");
            descriptor.put("descriptor_version", 1);
            descriptor.put("model_name", "java-smoke-model");
            descriptor.put("supports_streaming", false);
            var registration = client.registerLlmClient(
                    "java-smoke-llm", descriptor,
                    (call, cancellation) -> java.util.concurrent.CompletableFuture.completedFuture(
                            smokeOutcome()));
            var registrationHandle = registration.toCompletableFuture().get();
            AgentHandle agent = client.createAgent().toCompletableFuture().get();
            SessionHandle session = agent.createSession().toCompletableFuture().get();
            JsonSupport.fromWire(client.invoke(
                    "echo_core::agent::Agent::name", agent.wire(), List.of(session.wire()))
                    .toCompletableFuture().get());
            session.invoke("memory.store.put", List.of(List.of("sdk"), "java", "ok"))
                    .toCompletableFuture().get();
            var stored = session.invoke("memory.store.get", List.of(List.of("sdk"), "java"))
                    .toCompletableFuture().get();
            if (!"ok".equals(stored.path("value").asText())) {
                throw new IllegalStateException("unexpected memory family result: " + stored);
            }
            var telemetry = client.call("telemetry.status", null, List.of())
                    .toCompletableFuture().get();
            if (!telemetry.path("initialized").isBoolean()) {
                throw new IllegalStateException("unexpected telemetry status: " + telemetry);
            }
            var classified = client.classifyTurnOutcome(WireValues.variant(
                    "echo_sdk_protocol::methods::AgentEventWire",
                    "final_answer",
                    Map.of("text", "classified"))).toCompletableFuture().get();
            if (!"variant".equals(classified.path("kind").asText())
                    || !"completed".equals(classified.path("value").path("variant").asText())) {
                throw new IllegalStateException("unexpected TurnOutcome classification: " + classified);
            }
            var prompt = session.prompt("hello").toCompletableFuture().get();
            if (!"end_turn".equals(prompt.path("stopReason").asText())) {
                throw new IllegalStateException("unexpected Host prompt stop reason: " + prompt);
            }
            var run = session.startChat("run smoke").toCompletableFuture().get();
            if (!run.waitForSettlement().toCompletableFuture().get().path("settled").asBoolean()) {
                throw new IllegalStateException("Host run did not settle");
            }
            var runState = run.get().toCompletableFuture().get();
            if (!"completed".equals(runState.path("status").asText())) {
                throw new IllegalStateException("unexpected Host run status: " + runState);
            }
            var receiptStatus = run.status().toCompletableFuture().get();
            if (!"completed".equals(receiptStatus.asText())) {
                throw new IllegalStateException("unexpected TurnReceipt status: " + receiptStatus);
            }
            var outcomeStatus = run.outcomeStatus().toCompletableFuture().get();
            if (!"completed".equals(outcomeStatus.asText())) {
                throw new IllegalStateException("unexpected TurnOutcome status: " + outcomeStatus);
            }
            var usage = run.usage().toCompletableFuture().get();
            if (!usage.path("duration_ms").isTextual()) {
                throw new IllegalStateException("WireU64 duration_ms was not textual: " + usage);
            }
            for (String field : List.of("tokens_used", "iterations")) {
                var value = usage.path(field);
                if (!value.isMissingNode() && !value.isNull() && !value.isTextual()) {
                    throw new IllegalStateException("WireU64 " + field + " was not textual: " + usage);
                }
            }
            registrationHandle.unregister().toCompletableFuture().get();
            System.out.println("echo-agent Java SDK connected: " + agent.wire().id());
            session.close();
            agent.close();
        } finally {
            client.close();
        }
    }
}
