package com.echoagent.sdk;

import org.junit.jupiter.api.Test;

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
}
