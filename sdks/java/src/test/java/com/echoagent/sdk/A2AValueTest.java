package com.echoagent.sdk;

import org.junit.jupiter.api.Test;

import java.math.BigInteger;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class A2AValueTest {
    @Test
    void messageStatusProviderAndSkillValuesPreserveRustSemantics() {
        var message = A2AMessage.userText("hello");
        assertEquals("user", message.role());
        assertEquals("hello", message.textContent());
        assertEquals("agent", A2AMessage.agentText("answer").role());

        var status = A2ATaskStatus.withMessage(TaskState.WORKING, message);
        assertEquals(TaskState.WORKING, status.state());
        assertEquals("hello", status.message().textContent());
        assertTrue(status.timestamp().contains("T"));

        var provider = AgentProvider.newProvider("Echo").withUrl("https://example.test");
        assertEquals("Echo", provider.organization());
        assertEquals("https://example.test", provider.url());

        var skill = AgentSkill.newSkill("search", "Search docs")
                .withExamples(List.of("rust"))
                .withTags(List.of("docs"));
        assertEquals("search", skill.id());
        assertEquals(List.of("rust"), skill.examples());
        assertEquals(List.of("docs"), skill.tags());
        assertThrows(IllegalArgumentException.class, () -> A2AMessage.userText(null));
    }

    @Test
    void valueIdentityMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/a2a_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(14, count);
    }

    @Test
    void agentCardBuilderPreservesLocalValueSemantics() {
        var skill = AgentSkill.newSkill("search", "Search docs");
        var card = AgentCard.builder("eko", "https://example.test")
                .description("Local agent")
                .version("1.0.0")
                .provider(AgentProvider.newProvider("Echo"))
                .skill(skill)
                .inputModes(List.of("text/plain"))
                .outputModes(List.of("text/plain", "application/json"))
                .streaming()
                .pushNotifications()
                .build();
        assertEquals("eko", card.name());
        assertEquals("Local agent", card.description());
        assertEquals(List.of(skill), card.skills());
        assertEquals(List.of("text/plain", "application/json"), card.defaultOutputModes());
        assertTrue(card.capabilities().streaming());
        assertTrue(card.capabilities().pushNotifications());
    }

    @Test
    void agentCardIdentityMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/a2a_agent_card")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(14, count);
    }

    @Test
    void artifactAndErrorValuesPreserveWireFields() {
        var part = JsonSupport.MAPPER.createObjectNode().put("type", "text").put("text", "chunk");
        var artifact = A2AArtifact.newArtifact(List.of(part), "answer", BigInteger.valueOf(2), true);
        assertEquals("answer", artifact.name());
        assertEquals(BigInteger.valueOf(2), artifact.index());
        assertTrue(artifact.append());
        assertEquals("chunk", artifact.parts().get(0).path("text").asText());
        assertThrows(IllegalArgumentException.class, () -> A2AArtifact.newArtifact(
                List.of(JsonSupport.MAPPER.createObjectNode().put("type", "unknown")), null, null, false));
        var error = A2AError.newError(-32001, "missing");
        assertEquals(-32001, error.code());
        assertEquals("missing", error.message());
    }

    @Test
    void wireValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/a2a_wire_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(8, count);
    }

    @Test
    void streamValuesPreserveEventAndResponseSemantics() {
        var status = new TaskStatusUpdateEvent("task-1", A2ATaskStatus.newStatus(TaskState.WORKING), false);
        var artifact = new TaskArtifactUpdateEvent("task-1", A2AArtifact.newArtifact(
                List.of(JsonSupport.MAPPER.createObjectNode().put("type", "text").put("text", "chunk")),
                null, null, false), true);
        var response = A2AStreamResponse.newResponse("1", new A2AStreamEvent.StatusUpdate(status), null);
        assertEquals("task-1", status.taskId());
        assertTrue(artifact.isFinal());
        assertEquals("2.0", response.jsonrpc());
        assertTrue(response.result() instanceof A2AStreamEvent.StatusUpdate);
    }

    @Test
    void streamValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/a2a_stream_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(18, count);
    }

    @Test
    void taskEnvelopesPreserveNestedValueSemantics() {
        var message = A2AMessage.userText("hello");
        var params = A2ATaskParams.newParams(message, "task-1", "session-1");
        var request = A2ATaskRequest.newRequest("request-1", "tasks/send", params);
        var task = A2ATask.newTask("task-1", A2ATaskStatus.newStatus(TaskState.WORKING),
                "session-1", List.of(message), List.of());
        var response = A2ATaskResponse.newResponse("request-1", task, null);
        assertEquals("2.0", request.jsonrpc());
        assertEquals("hello", request.params().message().textContent());
        assertEquals("task-1", response.result().id());
    }

    @Test
    void taskEnvelopeMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/a2a_task_envelopes")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(15, count);
    }
}
