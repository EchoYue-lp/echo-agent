package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.math.BigInteger;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

class TypedExtensionTest {
    @Test
    void descriptorBuildersEmitTypedRegistrationSnapshots() {
        var tool = ToolDescriptor.builder()
                .name("search")
                .description("Search documents")
                .parametersValue(Map.of("type", "object"))
                .schemaRevision(BigInteger.valueOf(3))
                .supportsStreaming(true)
                .requiredPermissions(List.of("read"))
                .requiredInputModalities(List.of("text"))
                .build()
                .toJson();
        assertEquals("tool", tool.path("kind").asText());
        assertEquals("3", tool.path("schema_revision").asText());
        assertEquals("map", tool.path("parameters").path("kind").asText());

        var llm = LlmClientDescriptor.builder()
                .modelName("model")
                .supportsStreaming(true)
                .capabilities(LlmCapabilities.builder().imageInput(false).build())
                .build()
                .toJson();
        assertEquals("llm_client", llm.path("kind").asText());
        assertEquals(false, llm.path("capabilities").path("image_input").asBoolean());

        var store = StoreDescriptor.builder().searchModes(List.of("keyword", "hybrid")).build().toJson();
        assertEquals(List.of("keyword", "hybrid"),
                java.util.stream.StreamSupport.stream(store.path("search_modes").spliterator(), false)
                        .map(JsonNode::asText).toList());

        var critic = CriticDescriptor.builder().name("quality-check").build().toJson();
        assertEquals("critic", critic.path("kind").asText());
        assertEquals(1, critic.path("descriptor_version").asInt());
        assertEquals("quality-check", critic.path("name").asText());

        var compressor = ContextCompressorDescriptor.named("java-compressor").toJson();
        assertEquals("context_compressor", compressor.path("kind").asText());
        assertEquals(1, compressor.path("descriptor_version").asInt());
        assertEquals("java-compressor", compressor.path("name").asText());

        var component = AgentComponentDescriptor.of("audit_logger", "java-audit").toJson();
        assertEquals("agent_component", component.path("kind").asText());
        assertEquals("audit_logger", component.path("component").asText());

        var workflow = AgentComponentDescriptor.workflow("java-workflow", true).toJson();
        assertEquals(true, workflow.path("capabilities").path("supports_streaming").asBoolean());

        var policy = AgentComponentDescriptor.of("skill_load_policy", "java-policy").toJson();
        assertEquals("skill_load_policy", policy.path("component").asText());

        var checkpointStore = AgentComponentDescriptor.workflowCheckpointStore(
                "java-checkpoints", BigInteger.valueOf(1_000)).toJson();
        assertEquals("1000", checkpointStore.path("capabilities")
                .path("claim_heartbeat_interval_ms").asText());
        assertThrows(IllegalArgumentException.class,
                () -> AgentComponentDescriptor.of("workflow_checkpoint_store", "missing-heartbeat"));
        assertThrows(IllegalArgumentException.class,
                () -> AgentComponentDescriptor.workflowCheckpointStore(
                        "too-large", BigInteger.valueOf(300_001)));
    }

    @Test
    void callsPreserveIdentityDeadlineStreamAndTypedInput() {
        var call = JsonSupport.MAPPER.createObjectNode();
        call.set("extension", new WireHandle("extension-1", "2", "extension").toJson());
        call.put("invocation_id", "invocation-1");
        call.set("deadline", JsonSupport.MAPPER.createObjectNode().put("seconds", "20").put("nanos", 0));
        call.set("stream", new WireHandle("stream-1", "2", "stream").toJson());
        var invocation = call.putObject("invocation");
        invocation.put("operation", "tool_execute");
        invocation.putObject("input").set("parameters", WireValues.value(Map.of("q", "rust")));

        var typed = ToolCall.from(call);
        assertEquals("tool_execute", typed.operation());
        assertEquals("invocation-1", typed.invocationId());
        assertEquals("extension-1", typed.extension().id());
        assertEquals("stream-1", typed.stream().id());
        assertEquals("map", typed.parameters().path("kind").asText());
        assertNotNull(typed.deadline());

        var criticCall = call.deepCopy();
        criticCall.putObject("invocation")
                .put("operation", "critic_critique")
                .putObject("input")
                .put("task", "solve")
                .put("answer", "42")
                .put("context", "math");
        var typedCritic = CriticCall.from(criticCall);
        assertEquals("critic_critique", typedCritic.operation());
        assertEquals("solve", typedCritic.task());
        assertEquals("42", typedCritic.answer());
        assertEquals("math", typedCritic.critiqueContext());

        var compressionCall = call.deepCopy();
        var compressionInput = compressionCall.putObject("invocation")
                .put("operation", "compressor_compress")
                .putObject("input");
        compressionInput.putArray("messages").add(JsonSupport.MAPPER.createObjectNode()
                .put("role", "user").set("content", WireValues.value("hello")));
        compressionInput.put("token_limit", "4096");
        compressionInput.put("current_query", "summarize");
        compressionInput.putNull("focus_instructions");
        var tokenizer = compressionInput.putObject("tokenizer");
        tokenizer.set("resource", new WireHandle("tokenizer-1", "1", "facade_resource").toJson());
        tokenizer.put("owner_session_id", "session-1");
        var typedCompression = CompressionCall.from(compressionCall);
        assertEquals("compressor_compress", typedCompression.operation());
        assertEquals(BigInteger.valueOf(4096), typedCompression.tokenLimit());
        assertEquals("summarize", typedCompression.currentQuery());
        assertEquals(1, typedCompression.messages().size());

        var componentCall = call.deepCopy();
        var componentInput = componentCall.putObject("invocation")
                .put("operation", "agent_component_call")
                .putObject("input");
        componentInput.put("component", "audit_logger");
        var componentRequest = componentInput.putObject("call");
        componentRequest.put("operation", "audit_log");
        componentRequest.putObject("input").set("event", WireValues.value(Map.of("event", "fixture")));
        var typedComponent = AgentComponentCall.from(componentCall);
        assertEquals("audit_logger", typedComponent.component());
        assertEquals("audit_log", typedComponent.componentOperation());
        assertEquals(true, typedComponent.request() instanceof AgentComponentRequest.AuditLog);

        var unitComponentCall = componentCall.deepCopy();
        var unitInput = unitComponentCall.path("invocation").path("input");
        ((com.fasterxml.jackson.databind.node.ObjectNode) unitInput).put("component", "sandbox_executor");
        var unitRequest = ((com.fasterxml.jackson.databind.node.ObjectNode) unitInput).putObject("call");
        unitRequest.put("operation", "sandbox_is_available");
        var typedUnit = AgentComponentCall.from(unitComponentCall);
        assertEquals(true, typedUnit.request() instanceof AgentComponentRequest.SandboxIsAvailable);

        var embedComponentCall = componentCall.deepCopy();
        var embedInput = (com.fasterxml.jackson.databind.node.ObjectNode)
                embedComponentCall.path("invocation").path("input");
        embedInput.put("component", "embedder");
        var embedRequest = embedInput.putObject("call");
        embedRequest.put("operation", "embedder_embed");
        embedRequest.putObject("input").put("text", "hello");
        var typedEmbed = AgentComponentCall.from(embedComponentCall);
        assertEquals(true, typedEmbed.request() instanceof AgentComponentRequest.EmbedderEmbed);

        var streamComponentCall = componentCall.deepCopy();
        var streamInvocation = (com.fasterxml.jackson.databind.node.ObjectNode)
                streamComponentCall.path("invocation");
        streamInvocation.put("operation", "agent_component_call_stream");
        var streamInput = (com.fasterxml.jackson.databind.node.ObjectNode) streamInvocation.path("input");
        streamInput.put("component", "workflow");
        var streamRequest = streamInput.putObject("call");
        streamRequest.put("operation", "workflow_run_stream");
        streamRequest.putObject("input").put("input", "start");
        var typedStream = AgentComponentCall.from(streamComponentCall);
        assertEquals("agent_component_call_stream", typedStream.operation());
        assertEquals(true, typedStream.request() instanceof AgentComponentRequest.WorkflowRunStream);

        var checkpointCall = componentCall.deepCopy();
        var checkpointInput = (com.fasterxml.jackson.databind.node.ObjectNode)
                checkpointCall.path("invocation").path("input");
        checkpointInput.put("component", "workflow_checkpoint_store");
        var checkpointRequest = checkpointInput.putObject("call");
        checkpointRequest.put("operation", "workflow_checkpoint_save_if_generation");
        checkpointRequest.putObject("input")
                .set("checkpoint", WireValues.value(Map.of("status", "running")));
        ((com.fasterxml.jackson.databind.node.ObjectNode) checkpointRequest.path("input"))
                .put("expected_generation", "7");
        var typedCheckpoint = AgentComponentCall.from(checkpointCall);
        assertEquals(true, typedCheckpoint.request()
                instanceof AgentComponentRequest.WorkflowCheckpointSaveIfGeneration);

        var claimCall = componentCall.deepCopy();
        var claimInput = (com.fasterxml.jackson.databind.node.ObjectNode)
                claimCall.path("invocation").path("input");
        claimInput.put("component", "workflow_checkpoint_store");
        var claimRequest = claimInput.putObject("call");
        claimRequest.put("operation", "workflow_checkpoint_ack_claim");
        claimRequest.putObject("input")
                .put("checkpoint_id", "checkpoint-1")
                .put("attempt_id", "attempt-1");
        var typedClaim = AgentComponentCall.from(claimCall);
        assertEquals(true, typedClaim.request()
                instanceof AgentComponentRequest.WorkflowCheckpointAckClaim);
    }

    @Test
    void typedOutcomeBuildersUseCanonicalOperationDiscriminators() {
        var toolResult = ToolResult.text("done", true).build();
        var tool = ToolOutcome.result(toolResult).toJson();
        assertEquals("result", tool.path("outcome").asText());
        assertEquals("tool_execute", tool.path("result").path("operation").asText());
        assertEquals("text", tool.path("result").path("value").path("kind").path("kind").asText());

        var llmMessage = JsonSupport.MAPPER.createObjectNode()
                .put("role", "assistant")
                .set("content", WireValues.value("answer"));
        var llm = LlmOutcome.result(LlmChatResponse.builder()
                .message(llmMessage)
                .raw(Map.of("provider", "java"))
                .finishReason("stop")
                .build()).toJson();
        assertEquals("llm_chat", llm.path("result").path("operation").asText());

        var item = StoreItem.builder().key("k").namespace(List.of("n"))
                .value("v").createdAt(BigInteger.ONE).updatedAt(BigInteger.TWO).build();
        var stored = StoreOutcome.get(item).toJson();
        assertEquals("store_get", stored.path("result").path("operation").asText());
        assertEquals("k", stored.path("result").path("value").path("key").asText());
        assertEquals("18446744073709551615", StoreOutcome.pruned(JsonSupport.MAX_U64)
                .toJson().path("result").path("value").asText());

        var critic = CriticOutcome.result(Critique.builder()
                .score(8.5)
                .passed(true)
                .feedback("The answer is correct")
                .suggestions(List.of("Add a worked example"))
                .build()).toJson();
        assertEquals("critic_critique", critic.path("result").path("operation").asText());
        assertEquals(8.5, critic.path("result").path("value").path("score").asDouble());
        assertEquals(true, critic.path("result").path("value").path("passed").asBoolean());

        var message = JsonSupport.MAPPER.createObjectNode()
                .put("role", "assistant").set("content", WireValues.value("summary"));
        var compression = CompressionOutcome.result(CompressionOutput.builder()
                .messages(List.of(message))
                .evicted(List.of())
                .checkpoint(WireValues.value(Map.of("offset", "7")))
                .build()).toJson();
        assertEquals("compressor_compress", compression.path("result").path("operation").asText());
        assertEquals("assistant", compression.path("result").path("value")
                .path("messages").path(0).path("role").asText());

        var component = AgentComponentOutcome.result(
                "audit_logger", new AgentComponentResult.AuditLogged()).toJson();
        assertEquals("agent_component_call", component.path("result").path("operation").asText());
        assertEquals("audit_log", component.path("result").path("value")
                .path("result").path("operation").asText());

        var embedded = AgentComponentOutcome.result(
                "embedder", new AgentComponentResult.Embedded(List.of(0.25, 0.75))).toJson();
        assertEquals(0.75, embedded.path("result").path("value").path("result")
                .path("value").path("vector").path(1).asDouble());

        var policy = AgentComponentOutcome.result(
                "skill_load_policy", new AgentComponentResult.SkillLoadAllowed(false)).toJson();
        assertEquals(false, policy.path("result").path("value").path("result")
                .path("value").path("allowed").asBoolean());

        var committed = AgentComponentOutcome.result(
                "workflow_checkpoint_store",
                new AgentComponentResult.WorkflowCheckpointSavedIfGeneration(true)).toJson();
        assertEquals(true, committed.path("result").path("value").path("result")
                .path("value").path("committed").asBoolean());
        var acked = AgentComponentOutcome.result(
                "workflow_checkpoint_store",
                new AgentComponentResult.WorkflowCheckpointClaimAcked()).toJson();
        assertEquals("workflow_checkpoint_ack_claim", acked.path("result").path("value")
                .path("result").path("operation").asText());
    }

    @Test
    void agentComponentStreamsSeparateChunksFromTerminals() {
        var chunk = new AgentComponentStreamChunk.WorkflowNodeStart(
                "start", BigInteger.ZERO).toJson();
        assertEquals("workflow", chunk.path("component").asText());
        assertEquals("node_start", chunk.path("event").path("event").asText());

        var terminal = new AgentComponentStreamComplete.WorkflowCompleted(
                "done", BigInteger.ONE, BigInteger.ZERO, 1).toJson();
        assertEquals("done", terminal.path("terminal").path("result").asText());
        assertEquals("1", terminal.path("terminal").path("total_steps").asText());

        assertThrows(IllegalArgumentException.class,
                () -> new AgentComponentStreamComplete.SandboxFailed("timeout", "bad"));
    }

    @Test
    void typedBuildersRejectUnknownContractValues() {
        assertThrows(IllegalArgumentException.class,
                () -> ToolDescriptor.builder().name("x").description("x").riskLevel("unknown").build());
        assertThrows(IllegalArgumentException.class,
                () -> StoreDescriptor.builder().searchModes(List.of("semantic-ish")));
        assertThrows(IllegalArgumentException.class,
                () -> LlmChatCall.from(JsonSupport.MAPPER.createObjectNode()));
        assertThrows(IllegalArgumentException.class,
                () -> CriticCall.from(JsonSupport.MAPPER.createObjectNode()));
        assertThrows(IllegalArgumentException.class,
                () -> CompressionCall.from(JsonSupport.MAPPER.createObjectNode()));
        var malformedCompression = JsonSupport.MAPPER.createObjectNode();
        malformedCompression.set("extension", new WireHandle("extension-1", "1", "extension").toJson());
        malformedCompression.put("invocation_id", "compress-invalid");
        malformedCompression.set("deadline",
                JsonSupport.MAPPER.createObjectNode().put("seconds", "20").put("nanos", 0));
        var malformedCompressionInput = malformedCompression.putObject("invocation")
                .put("operation", "compressor_compress")
                .putObject("input");
        malformedCompressionInput.putArray("messages");
        malformedCompressionInput.put("token_limit", "04096");
        assertThrows(IllegalArgumentException.class, () -> CompressionCall.from(malformedCompression));
        assertThrows(IllegalArgumentException.class,
                () -> Critique.builder().score(Double.NaN).feedback("x").build());
        assertThrows(IllegalArgumentException.class,
                () -> Critique.builder().score(10.1).feedback("x").build());
        assertThrows(IllegalArgumentException.class,
                () -> AgentComponentOutcome.result(
                        "run_store", new AgentComponentResult.Embedded(List.of(1.0))));
    }
}
