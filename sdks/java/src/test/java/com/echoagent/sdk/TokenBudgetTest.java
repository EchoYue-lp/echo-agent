package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class TokenBudgetTest {
    @Test
    void tokenBudgetAndTimeoutPoliciesPreserveSemantics() {
        TokenBudget budget = TokenBudget.newBudget(100_000);
        assertEquals(10_000, budget.systemPromptBudget());
        assertEquals(5_000, budget.toolDefinitionsBudget());
        assertEquals(65_000, budget.conversationBudget());
        TokenAllocation allocation = budget.allocate(5_000, 2_000, 75_000);
        assertTrue(!allocation.ok());
        assertTrue(allocation.needsCompression());
        assertEquals(2_000, allocation.conversationExcess());
        assertEquals(80_000, budget.withAllocations(.05, .05, .05, .05).conversationBudget());
        assertThrows(IllegalArgumentException.class, () -> budget.withAllocations(.8, .3, 0, 0));
        assertTrue(!TokenBudgetConfig.disabled().isEnabled());
        assertEquals(10_000, TokenBudgetConfig.enabled().withTotalWindow(10_000).build(1_000).totalWindow());
        assertEquals(null, LlmTimeouts.defaults().withoutIdleTimeout().idleTimeout());
        assertEquals(null, LlmTimeouts.defaults().withOverallTimeout(0).overallTimeout());
    }

    @Test
    void tokenBudgetMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/token_budget_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(35, count);
    }
}
