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

    @Test
    void thinkingLevelParsesRustAliases() {
        assertEquals(ThinkingLevel.NONE, ThinkingLevel.parse(" OFF "));
        assertEquals(ThinkingLevel.MEDIUM, ThinkingLevel.parse("normal"));
        assertEquals(ThinkingLevel.XHIGH, ThinkingLevel.parse("xhigh"));
        assertEquals(null, ThinkingLevel.parse("unknown"));
    }

    @Test
    void thinkingLevelMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/thinking_level")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(9, count);
    }

    @Test
    void steeringValuesPreserveLifecycleAndTerminalSemantics() {
        var accepted = AgentSteerState.accepted();
        var drained = AgentSteerState.drained();
        var settled = AgentSteerState.turnSettled(AgentSteerTurnOutcome.COMPLETED, true);
        assertEquals(AgentSteerPhase.ACCEPTED, accepted.phase());
        assertEquals(false, accepted.wasDrained());
        assertEquals(AgentSteerPhase.DRAINED, drained.phase());
        assertEquals(true, drained.wasDrained());
        assertEquals(AgentSteerPhase.TURN_SETTLED, settled.phase());
        assertEquals(AgentSteerTurnOutcome.COMPLETED, ((AgentSteerState.TurnSettled) settled).outcome());
        assertEquals(true, settled.wasDrained());
        assertEquals("failed", AgentSteerTurnOutcome.FAILED.asStr());
        assertEquals(AgentSteerTurnOutcome.CANCELLED, AgentSteerTurnOutcome.parse("cancelled"));
        assertEquals(null, AgentSteerTurnOutcome.parse("unknown"));
    }

    @Test
    void steeringValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/steering_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(13, count);
    }

    @Test
    void subagentValuesPreserveStablePhaseAndStatusSemantics() {
        assertEquals("mailbox_accepted", SubagentCommandPhase.MAILBOX_ACCEPTED.asStr());
        assertEquals(SubagentCommandPhase.TURN_SETTLED, SubagentCommandPhase.parse("turn_settled"));
        assertEquals(null, SubagentCommandPhase.parse("unknown"));
        assertEquals("timed_out", SubagentStatus.TIMED_OUT.asStr());
        assertEquals(SubagentStatus.COMPLETED, SubagentStatus.parse("completed"));
        var error = org.junit.jupiter.api.Assertions.assertThrows(
                IllegalArgumentException.class, () -> SubagentStatus.parse("unknown"));
        assertTrue(error.getMessage().contains("unknown Subagent status"));
    }

    @Test
    void subagentValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/subagent_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(15, count);
    }

    @Test
    void contentGuardValuesPreserveVariantPayloads() {
        assertEquals(false, ContentGuardResult.pass().isRejected());
        assertEquals(java.util.List.of("email"),
                ((ContentGuardResult.Detected) ContentGuardResult.detected(java.util.List.of("email")))
                        .piiTypes());
        assertEquals(true, ContentGuardResult.rejected(java.util.List.of("phone")).isRejected());
        assertEquals(false, ContentGuardResult.redacted("safe").isRejected());
    }

    @Test
    void contentGuardValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/content_guard_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void guardDecisionsPreserveVariantPayloads() {
        assertEquals(false, GuardDecision.pass().isBlocked());
        assertEquals(true, GuardDecision.block("unsafe").isBlocked());
        var warned = (GuardDecision.Warn) GuardDecision.warn(java.util.List.of("one"));
        assertEquals(java.util.List.of("one"), warned.reasons());
        var transformed = (GuardDecision.Transform) GuardDecision.transform("safe", java.util.List.of("redacted"));
        assertEquals("safe", transformed.content());
        assertEquals(false, transformed.isBlocked());
    }

    @Test
    void guardDecisionMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/guard_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void deliveryValuesPreserveStableSnakeCaseSpellings() {
        assertEquals("outcome_unknown", DeliveryOutcome.OUTCOME_UNKNOWN.asStr());
        assertEquals("effect_started", DeliveryPhase.EFFECT_STARTED.asStr());
        assertEquals("turn_settled", DeliveryPhase.TURN_SETTLED.asStr());
    }

    @Test
    void deliveryValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/delivery_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(16, count);
    }

    @Test
    void subagentStopStatusesPreserveStableHookSpellings() {
        assertEquals("timed_out", SubagentStopStatus.TIMED_OUT.asStr());
        assertEquals("completed", SubagentStopStatus.COMPLETED.toString());
    }

    @Test
    void subagentStopValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/subagent_stop_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void taskTerminalStatusesPreserveStableHookSpellings() {
        assertEquals("timed_out", TaskTerminalStatus.TIMED_OUT.asStr());
        assertEquals("skipped", TaskTerminalStatus.SKIPPED.toString());
    }

    @Test
    void taskTerminalValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/task_terminal_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }

    @Test
    void permissionRuleSourcesPreserveCanonicalAndAliasParsing() {
        assertEquals("session", RuleSource.SESSION.toString());
        assertEquals(RuleSource.LOCAL_SETTINGS, RuleSource.parse("local_settings"));
        assertEquals(RuleSource.USER_SETTINGS, RuleSource.parse("manual"));
        assertThrows(IllegalArgumentException.class, () -> RuleSource.parse("unknown"));
    }

    @Test
    void permissionRuleSourceMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/permission_rule_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(10, count);
    }

    @Test
    void permissionRuleBehaviorPreservesParseDefaultsAndDecisions() {
        assertTrue(RuleBehavior.parse("allow") instanceof RuleBehavior.Allow);
        var deny = (RuleBehavior.Deny) RuleBehavior.parse("deny");
        assertEquals("denied by rule", deny.reason());
        var ask = (RuleBehavior.Ask) RuleBehavior.parse("ask");
        assertEquals(java.util.List.of("allow", "deny"), ask.suggestions());
        assertThrows(IllegalArgumentException.class, () -> RuleBehavior.parse("unknown"));
    }

    @Test
    void permissionRuleBehaviorMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/permission_rule_behavior")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void permissionModesPreserveAliasesAndHelperSemantics() {
        assertEquals(PermissionMode.ACCEPT_EDITS, PermissionMode.parse("autoedit"));
        assertEquals(PermissionMode.DEFAULT, PermissionMode.parse("ask"));
        assertEquals(true, PermissionMode.ACCEPT_EDITS.allowsWrite());
        assertEquals(true, PermissionMode.STRICT_CONFIRM.requiresInteraction());
        assertEquals(true, PermissionMode.AUTO.usesClassifier());
        assertThrows(IllegalArgumentException.class, () -> PermissionMode.parse("unknown"));
    }

    @Test
    void permissionModeHelperMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/permission_mode_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void permissionRuleMatchersPreserveParseDisplayMatching() {
        var pattern = RuleMatcher.parse("pattern:Bash(rm:*)");
        assertEquals(true, pattern.matches("Bash(rm:rf)", java.util.List.of()));
        assertEquals(true, RuleMatcher.parse("pattern:*").matches("anything", java.util.List.of()));
        assertEquals(true, RuleMatcher.parse("pattern:Bash(rm:?f)").matches("Bash(rm:rf)", java.util.List.of()));
        assertEquals(true, RuleMatcher.parse("pattern:Bash(*:*)").matches("Bash(git:status)", java.util.List.of()));
        assertEquals(true, pattern.matchesMatcherStr("Bash(rm:*)"));
        assertEquals("permission:read", RuleMatcher.parse("perm:read").display());
        assertThrows(IllegalArgumentException.class, () -> RuleMatcher.parse("unknown"));
        assertThrows(IllegalArgumentException.class, () -> RuleMatcher.parse("perm:READ"));
    }

    @Test
    void permissionRuleMatcherMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/permission_rule_matcher")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(9, count);
    }

    @Test
    void commandCellPhasesPreserveStableSpellingAndTerminalSemantics() {
        assertEquals("launch_failed", CommandCellPhase.LAUNCH_FAILED.asStr());
        assertEquals(false, CommandCellPhase.RUNNING.isTerminal());
        assertEquals(true, CommandCellPhase.SUCCEEDED.isTerminal());
    }

    @Test
    void commandCellPhaseMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/command_cell_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(10, count);
    }

    @Test
    void commandCellTerminalArtifactValuesPreserveSpellings() {
        assertEquals("output_drain_failed", CommandCellStatus.TerminalCause.OUTPUT_DRAIN_FAILED.asStr());
        assertEquals("below_threshold", CommandCellStatus.ArtifactStatus.BELOW_THRESHOLD.asStr());
    }

    @Test
    void commandCellTerminalArtifactMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/command_cell_status_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(15, count);
    }

    @Test
    void teamStrategiesPreserveValuesAndDescriptions() {
        var strategy = TeamStrategy.debate("judge", java.util.List.of("a", "b"));
        assertEquals("debate", strategy.name());
        assertEquals(java.util.List.of("a", "b"), ((TeamStrategy.Debate) strategy).debaters());
        assertEquals("Debaters propose independently and a judge synthesizes", strategy.description());
    }

    @Test
    void teamStrategyMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/team_strategy_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }

    @Test
    void acpRuntimeValuesPreserveSettlementAndLedgerLimits() {
        assertEquals("standard", AcpValues.ConnectionMode.STANDARD.asStr());
        assertEquals("timed_out", AcpValues.ExtensionSettlement.TIMED_OUT.asStr());
        assertTrue(AcpValues.ExtensionSettlement.ANSWERED.isAnswered());
        assertEquals(false, AcpValues.ExtensionSettlement.CANCELLED.isAnswered());
        assertEquals(new AcpValues.AcpLedgerLimits(10_000L, 8L * 1024L * 1024L),
                AcpValues.AcpLedgerLimits.defaults());
        assertThrows(IllegalArgumentException.class, () -> new AcpValues.AcpLedgerLimits(-1, 1));
    }

    @Test
    void acpRuntimeValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/acp_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(13, count);
    }

    @Test
    void acpAdapterConfigPreservesDefaultsAndValidation() {
        var config = AcpAdapterConfig.defaults();
        config.validate();
        assertEquals(128L, config.maxSessions());
        assertEquals(java.time.Duration.ofSeconds(5), config.shutdownTimeout());
        var invalid = new AcpAdapterConfig("", config.title(), config.version(), config.maxSessions(),
                config.maxPromptChars(), config.maxUpdateChars(), config.maxUpdatesPerTurn(),
                config.maxTotalUpdateChars(), config.maxExtensionConcurrency(), config.shutdownTimeout());
        assertThrows(IllegalArgumentException.class, invalid::validate);
    }

    @Test
    void acpAdapterConfigMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/acp_config_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(13, count);
    }

    @Test
    void acpLeaseErrorsPreserveDisplayText() {
        assertEquals("extension admission is closed", AcpValues.ExtensionLeaseError.ADMISSION_CLOSED.asStr());
        assertEquals("extension concurrency limit reached", AcpValues.ExtensionLeaseError.CONCURRENCY_LIMIT.toString());
    }

    @Test
    void acpLeaseErrorMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/acp_lease_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(5, count);
    }

    @Test
    void jwtConfigAndClaimsPreserveLocalValues() {
        var config = JwtConfig.hs256("secret").withIssuer("echo-agent").withAudience("a2a");
        assertTrue(config.isEnabled());
        assertTrue(config.toString().contains("verification_key: [redacted]"));
        assertEquals(false, JwtConfig.disabled().isEnabled());
        assertEquals("subject", new JwtClaims(null, "subject", null, null, null, null, null, java.util.Map.of()).subject());
        assertThrows(IllegalArgumentException.class, () -> JwtConfig.rs256("invalid"));
    }

    @Test
    void jwtMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/jwt_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(9, count);
    }

    @Test
    void dependencyAndSkillSourceValuesPreserveSpellings() {
        assertEquals("python_pkg", DependencyKind.PYTHON_PKG.asStr());
        assertEquals("node_module", DependencyKind.NODE_MODULE.toString());
        assertEquals("mcp", SkillSource.MCP.asStr());
    }

    @Test
    void dependencyValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/dependency_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }

    @Test
    void contextInheritancePreservesDefaults() {
        assertEquals(null, ContextInheritance.freshDefault().inheritHistory());
        assertEquals(true, ContextInheritance.forkDefault().inheritMemory());
        assertEquals(java.util.List.of(), ContextInheritance.forMode("team").inheritTools());
    }

    @Test
    void contextInheritanceMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/context_inheritance_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(11, count);
    }

    @Test
    void observedIsolationPreservesTrimDefaultAndUnicodeBounds() {
        assertEquals("worktree", ObservedIsolation.newValue("  worktree  ").asStr());
        assertEquals("unknown", ObservedIsolation.newValue("   ").asStr());
        String value = ObservedIsolation.newValue("😀".repeat(600)).asStr();
        assertEquals(512, value.codePointCount(0, value.length()));
    }

    @Test
    void observedIsolationMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/observed_isolation_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(4, count);
    }

    @Test
    void segmentRangesPreserveSaturatingHalfOpenLength() {
        assertEquals(3L, new SegmentRange(2L, 5L).len());
        assertEquals(0L, new SegmentRange(5L, 2L).len());
        assertEquals(true, new SegmentRange(5L, 2L).isEmpty());
    }

    @Test
    void segmentRangeMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/segment_range_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(3, count);
    }

    @Test
    void promptDiagnosticsPreserveRecordAndCountSemantics() {
        var diagnostics = new PromptDiagnostics();
        diagnostics.record("system", "base");
        diagnostics.record("system", "overlay");
        assertEquals(2L, diagnostics.count("system"));
        assertEquals(0L, diagnostics.count("missing"));
    }

    @Test
    void promptDiagnosticsMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/prompt_diagnostics_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(3, count);
    }

    @Test
    void subagentCommandIdentitiesPreserveValidationAndAttemptProjection() {
        var identity = new SubagentCommandIdentity("run", "task", "exec", 2L, 0L, "command");
        assertEquals("exec", identity.attemptIdentity().executionId());
        assertThrows(IllegalArgumentException.class,
                () -> new SubagentCommandIdentity("", "task", "exec", 2L, 0L, "command"));
        assertThrows(IllegalArgumentException.class,
                () -> new SubagentCommandIdentity("run", "task", "exec", 0L, 0L, "command"));
    }

    @Test
    void subagentCommandIdentityMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/subagent_command_identity_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void subagentUsageAccumulatesAndEmitsPayload() {
        var stats = new LlmUsageStats();
        stats.record("model", 100L, 50L, 150L, 80L, 10L, true);
        stats.record("model", 200L, 60L, 260L, 150L, 20L, false);
        var payload = stats.toPayload("session");
        assertEquals(300L, payload.get("prompt_tokens"));
        assertEquals(2L, payload.get("call_count"));
        assertEquals(true, payload.get("usage_reported"));
    }

    @Test
    void subagentUsageMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/subagent_usage_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(3, count);
    }

    @Test
    void artifactConfigPreservesDefaultsAndImmutableBuilders() {
        var config = ToolOutputArtifactConfig.newConfig("/tmp/artifacts", "temporary_1h")
                .thresholdBytes(0L).maxAgeSecs(60L);
        assertEquals(1L, config.thresholdBytes());
        assertEquals(60L, config.maxAgeSecs());
        assertEquals(3_600L, ToolOutputArtifactConfig.defaults().maxAgeSecs());
    }

    @Test
    void artifactConfigMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/tool_output_artifact_config_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }

    @Test
    void skillValidationReportsPreserveViolationGateSemantics() {
        assertEquals(true, new SkillValidationReport("skill", java.util.List.of(), java.util.List.of()).isValid());
        assertEquals(false, new SkillValidationReport("skill", java.util.List.of("missing name"), java.util.List.of()).isValid());
    }

    @Test
    void skillValidationMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/skill_validation_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(2, count);
    }

    @Test
    void skillContentPreservesStructuredPromptBlock() {
        var content = new SkillContent("demo", "/tmp/demo", "  Do the thing.  ",
                java.util.List.of("read"), java.util.List.of(new SkillContent.SkillResourceEntry("script", "scripts/run.sh")));
        var block = content.toPromptBlock();
        assertTrue(block.contains("<skill_content name=\"demo\">")
                && block.contains("<allowed_tools>")
                && block.contains("<file kind=\"script\">scripts/run.sh</file>"));
    }

    @Test
    void skillContentMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/skill_content_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(2, count);
    }

    @Test
    void jsonrpcValuesPreserveMcpConstructors() {
        var request = JsonRpcValues.Request.newRequest("tools/list", null);
        assertEquals("2.0", request.jsonrpc());
        assertEquals(null, request.id());
        assertEquals("notifications/initialized", JsonRpcValues.Notification
                .newNotification("notifications/initialized", java.util.Map.of("ok", true)).method());
    }

    @Test
    void jsonrpcValueMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/jsonrpc_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(4, count);
    }

    @Test
    void subagentContextPreservesEmptyAndContentSemantics() {
        assertEquals(false, SubagentContext.empty().hasContent());
        assertEquals(true, new SubagentContext(java.util.List.of(), java.util.List.of(), false, "goal", null).hasContent());
    }

    @Test
    void subagentContextMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/subagent_context_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(3, count);
    }

    @Test
    void usagePreservesCachePriorityAndEffectiveTotals() {
        var usage = new Usage(100L, 20L, null, null, null, null, null, 30L, null, null);
        assertEquals(30L, usage.cachedPromptTokens());
        assertEquals(130L, usage.effectivePromptTokens());
        assertEquals(150L, usage.effectiveTotalTokens());
        assertEquals(30.0 / 130.0, usage.cacheHitRate());
        assertThrows(IllegalArgumentException.class,
                () -> new Usage(0x1_0000_0000L, null, null, null, null, null, null, null, null, null));
    }

    @Test
    void usageMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/usage_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }

    @Test
    void hookActionsPreserveTaggedValuesAndValidation() {
        var command = new HookAction.Command("echo ok", null, 300L);
        assertEquals("command", command.kind());
        command.validate();
        new HookAction.Permission("ask", null, java.util.List.of()).validate();
        new HookAction.Http("http://localhost:8080/hook", null, null, 300L).validate();
        assertThrows(IllegalArgumentException.class, () -> new HookAction.Permission("maybe", null, java.util.List.of()).validate());
    }

    @Test
    void hookActionMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/hook_action_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(30, count);
    }

    @Test
    void pageInfoAppliesMetadataTruncationAndContinuation() {
        var result = new PageInfo("cursor-2", true, true, 4L, 2L).applyTo(ToolResult.success("items"));
        assertEquals(true, result.toJson().path("truncated").asBoolean());
        assertEquals("cursor-2", result.toJson().path("metadata").path("page.next_cursor").asText());
        assertTrue(result.toJson().path("output").asText().contains("[page]"));
    }

    @Test
    void pageInfoMappingsAreReady() throws Exception {
        var manifest = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(
                java.nio.file.Path.of("../..", "contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (var entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/page_info_values")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(2, count);
    }
}
