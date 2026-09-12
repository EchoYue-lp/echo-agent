package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;
import java.util.List;

/** Closed result hierarchy for every Agent component callback operation. */
public sealed interface AgentComponentResult permits
        AgentComponentResult.ConversationCreated,
        AgentComponentResult.ConversationFound,
        AgentComponentResult.ConversationsListed,
        AgentComponentResult.ConversationUpdated,
        AgentComponentResult.ConversationDeleted,
        AgentComponentResult.ConversationMessagesSaved,
        AgentComponentResult.ConversationMessagesLoaded,
        AgentComponentResult.ConversationMessagesCounted,
        AgentComponentResult.ConversationEnsured,
        AgentComponentResult.ConversationsSearched,
        AgentComponentResult.RunSaved,
        AgentComponentResult.RunLoaded,
        AgentComponentResult.SessionRunsListed,
        AgentComponentResult.RunsListed,
        AgentComponentResult.RunEventAppended,
        AgentComponentResult.ParentRunsListed,
        AgentComponentResult.RuntimeCheckpointLoaded,
        AgentComponentResult.RuntimeCheckpointSaved,
        AgentComponentResult.RuntimeScopeCheckpointSaved,
        AgentComponentResult.RuntimeStateIdsLoaded,
        AgentComponentResult.RuntimeStateCleared,
        AgentComponentResult.RuntimeScopeCleared,
        AgentComponentResult.RuntimeConversationCleared,
        AgentComponentResult.AuditLogged,
        AgentComponentResult.AuditEventsQueried,
        AgentComponentResult.ContextProjected,
        AgentComponentResult.MemoryTriggered,
        AgentComponentResult.GuardChecked,
        AgentComponentResult.SearchCompleted,
        AgentComponentResult.WorkflowCheckpointSaved,
        AgentComponentResult.WorkflowCheckpointLoaded,
        AgentComponentResult.WorkflowCheckpointClaimed,
        AgentComponentResult.WorkflowCheckpointsListed,
        AgentComponentResult.WorkflowCheckpointDeleted,
        AgentComponentResult.WorkflowCheckpointsCleared,
        AgentComponentResult.RevisionedTaskLoaded,
        AgentComponentResult.RevisionedTaskCommitted,
        AgentComponentResult.SandboxAvailability,
        AgentComponentResult.SandboxExecuted,
        AgentComponentResult.SandboxExecutedWithLimits,
        AgentComponentResult.SandboxExecutedWithLimitsAndCancel,
        AgentComponentResult.SandboxCleaned,
        AgentComponentResult.McpTransportSent,
        AgentComponentResult.McpTransportNotified,
        AgentComponentResult.McpTransportClosed,
        AgentComponentResult.McpTransportNotification,
        AgentComponentResult.Embedded,
        AgentComponentResult.MemoryPromoted,
        AgentComponentResult.WorkflowCompleted,
        AgentComponentResult.IntentClassified,
        AgentComponentResult.SkillLoadAllowed {

    String operation();
    JsonNode value();

    record ConversationCreated(JsonNode conversation) implements AgentComponentResult {
        @Override public String operation() { return "conversation_create"; }
        @Override public JsonNode value() { return object("conversation", conversation); }
    }
    record ConversationFound(JsonNode conversation) implements AgentComponentResult {
        @Override public String operation() { return "conversation_get"; }
        @Override public JsonNode value() { return nullableObject("conversation", conversation); }
    }
    record ConversationsListed(List<JsonNode> conversations) implements AgentComponentResult {
        @Override public String operation() { return "conversation_list"; }
        @Override public JsonNode value() { return array("conversations", conversations); }
    }
    record ConversationUpdated() implements AgentComponentResult {
        @Override public String operation() { return "conversation_update"; }
        @Override public JsonNode value() { return null; }
    }
    record ConversationDeleted() implements AgentComponentResult {
        @Override public String operation() { return "conversation_delete"; }
        @Override public JsonNode value() { return null; }
    }
    record ConversationMessagesSaved() implements AgentComponentResult {
        @Override public String operation() { return "conversation_save_messages"; }
        @Override public JsonNode value() { return null; }
    }
    record ConversationMessagesLoaded(List<JsonNode> messages) implements AgentComponentResult {
        @Override public String operation() { return "conversation_get_messages"; }
        @Override public JsonNode value() { return array("messages", messages); }
    }
    record ConversationMessagesCounted(BigInteger count) implements AgentComponentResult {
        @Override public String operation() { return "conversation_count_messages"; }
        @Override public JsonNode value() {
            return JsonSupport.MAPPER.createObjectNode().put(
                    "count", TypedExtensionSupport.canonicalU64(count.toString(), "count"));
        }
    }
    record ConversationEnsured(JsonNode conversation) implements AgentComponentResult {
        @Override public String operation() { return "conversation_ensure"; }
        @Override public JsonNode value() { return object("conversation", conversation); }
    }
    record ConversationsSearched(List<JsonNode> conversations) implements AgentComponentResult {
        @Override public String operation() { return "conversation_search"; }
        @Override public JsonNode value() { return array("conversations", conversations); }
    }
    record RunSaved() implements AgentComponentResult {
        @Override public String operation() { return "run_save"; }
        @Override public JsonNode value() { return null; }
    }
    record RunLoaded(JsonNode run) implements AgentComponentResult {
        @Override public String operation() { return "run_load"; }
        @Override public JsonNode value() { return nullableObject("run", run); }
    }
    record SessionRunsListed(List<JsonNode> runs) implements AgentComponentResult {
        @Override public String operation() { return "run_list_by_session"; }
        @Override public JsonNode value() { return array("runs", runs); }
    }
    record RunsListed(List<JsonNode> runs) implements AgentComponentResult {
        @Override public String operation() { return "run_list_all"; }
        @Override public JsonNode value() { return array("runs", runs); }
    }
    record RunEventAppended() implements AgentComponentResult {
        @Override public String operation() { return "run_append_event"; }
        @Override public JsonNode value() { return null; }
    }
    record ParentRunsListed(List<JsonNode> runs) implements AgentComponentResult {
        @Override public String operation() { return "run_list_by_parent"; }
        @Override public JsonNode value() { return array("runs", runs); }
    }
    record RuntimeCheckpointLoaded(JsonNode checkpoint) implements AgentComponentResult {
        @Override public String operation() { return "runtime_get_checkpoint"; }
        @Override public JsonNode value() { return nullableObject("checkpoint", checkpoint); }
    }
    record RuntimeCheckpointSaved() implements AgentComponentResult {
        @Override public String operation() { return "runtime_save_checkpoint"; }
        @Override public JsonNode value() { return null; }
    }
    record RuntimeScopeCheckpointSaved() implements AgentComponentResult {
        @Override public String operation() { return "runtime_save_checkpoint_for_scope"; }
        @Override public JsonNode value() { return null; }
    }
    record RuntimeStateIdsLoaded(List<String> stateIds) implements AgentComponentResult {
        @Override public String operation() { return "runtime_state_ids"; }
        @Override public JsonNode value() {
            var value = JsonSupport.MAPPER.createObjectNode();
            var stateIdsValue = value.putArray("state_ids");
            stateIds.forEach(stateIdsValue::add);
            return value;
        }
    }
    record RuntimeStateCleared(JsonNode receipt) implements AgentComponentResult {
        @Override public String operation() { return "runtime_clear_state"; }
        @Override public JsonNode value() { return object("receipt", receipt); }
    }
    record RuntimeScopeCleared(JsonNode receipt) implements AgentComponentResult {
        @Override public String operation() { return "runtime_clear_scope"; }
        @Override public JsonNode value() { return object("receipt", receipt); }
    }
    record RuntimeConversationCleared() implements AgentComponentResult {
        @Override public String operation() { return "runtime_clear_conversation"; }
        @Override public JsonNode value() { return null; }
    }
    record AuditLogged() implements AgentComponentResult {
        @Override public String operation() { return "audit_log"; }
        @Override public JsonNode value() { return null; }
    }
    record AuditEventsQueried(List<JsonNode> events) implements AgentComponentResult {
        @Override public String operation() { return "audit_query"; }
        @Override public JsonNode value() { return array("events", events); }
    }
    record ContextProjected(List<JsonNode> projections) implements AgentComponentResult {
        @Override public String operation() { return "context_project"; }
        @Override public JsonNode value() { return array("projections", projections); }
    }
    record MemoryTriggered(String disposition) implements AgentComponentResult {
        public MemoryTriggered {
            if (!"persist".equals(disposition) && !"captured".equals(disposition)) {
                throw new IllegalArgumentException("disposition must be persist or captured");
            }
        }
        @Override public String operation() { return "memory_trigger"; }
        @Override public JsonNode value() {
            return JsonSupport.MAPPER.createObjectNode().put("disposition", disposition);
        }
    }
    record GuardChecked(JsonNode result) implements AgentComponentResult {
        @Override public String operation() { return "guard_check"; }
        @Override public JsonNode value() { return object("result", result); }
    }
    record SearchCompleted(List<JsonNode> results) implements AgentComponentResult {
        @Override public String operation() { return "search_provider_search"; }
        @Override public JsonNode value() { return array("results", results); }
    }
    record WorkflowCheckpointSaved() implements AgentComponentResult {
        @Override public String operation() { return "workflow_checkpoint_save"; }
        @Override public JsonNode value() { return null; }
    }
    record WorkflowCheckpointLoaded(JsonNode checkpoint) implements AgentComponentResult {
        @Override public String operation() { return "workflow_checkpoint_load"; }
        @Override public JsonNode value() { return nullableObject("checkpoint", checkpoint); }
    }
    record WorkflowCheckpointClaimed(JsonNode checkpoint) implements AgentComponentResult {
        @Override public String operation() { return "workflow_checkpoint_claim"; }
        @Override public JsonNode value() { return nullableObject("checkpoint", checkpoint); }
    }
    record WorkflowCheckpointsListed(String operation, List<JsonNode> checkpoints)
            implements AgentComponentResult {
        public WorkflowCheckpointsListed {
            if (!java.util.Set.of("workflow_checkpoint_list", "workflow_checkpoint_list_by_graph",
                    "workflow_checkpoint_list_filtered").contains(operation)) {
                throw new IllegalArgumentException("invalid workflow checkpoint list operation");
            }
        }
        @Override public JsonNode value() { return array("checkpoints", checkpoints); }
    }
    record WorkflowCheckpointDeleted() implements AgentComponentResult {
        @Override public String operation() { return "workflow_checkpoint_delete"; }
        @Override public JsonNode value() { return null; }
    }
    record WorkflowCheckpointsCleared() implements AgentComponentResult {
        @Override public String operation() { return "workflow_checkpoint_clear"; }
        @Override public JsonNode value() { return null; }
    }
    record RevisionedTaskLoaded(JsonNode graph) implements AgentComponentResult {
        @Override public String operation() { return "revisioned_task_load"; }
        @Override public JsonNode value() { return nullableObject("graph", graph); }
    }
    record RevisionedTaskCommitted(JsonNode graph) implements AgentComponentResult {
        @Override public String operation() { return "revisioned_task_compare_and_commit"; }
        @Override public JsonNode value() { return object("graph", graph); }
    }
    record SandboxAvailability(boolean available) implements AgentComponentResult {
        @Override public String operation() { return "sandbox_is_available"; }
        @Override public JsonNode value() {
            return JsonSupport.MAPPER.createObjectNode().put("available", available);
        }
    }
    record SandboxExecuted(JsonNode result) implements AgentComponentResult {
        @Override public String operation() { return "sandbox_execute"; }
        @Override public JsonNode value() { return object("result", result); }
    }
    record SandboxExecutedWithLimits(JsonNode result) implements AgentComponentResult {
        @Override public String operation() { return "sandbox_execute_with_limits"; }
        @Override public JsonNode value() { return object("result", result); }
    }
    record SandboxExecutedWithLimitsAndCancel(JsonNode result) implements AgentComponentResult {
        @Override public String operation() { return "sandbox_execute_with_limits_and_cancel"; }
        @Override public JsonNode value() { return object("result", result); }
    }
    record SandboxCleaned() implements AgentComponentResult {
        @Override public String operation() { return "sandbox_cleanup"; }
        @Override public JsonNode value() { return null; }
    }
    record McpTransportSent(JsonNode response) implements AgentComponentResult {
        @Override public String operation() { return "mcp_transport_send"; }
        @Override public JsonNode value() { return object("response", response); }
    }
    record McpTransportNotified() implements AgentComponentResult {
        @Override public String operation() { return "mcp_transport_notify"; }
        @Override public JsonNode value() { return null; }
    }
    record McpTransportClosed() implements AgentComponentResult {
        @Override public String operation() { return "mcp_transport_close"; }
        @Override public JsonNode value() { return null; }
    }
    record McpTransportNotification(JsonNode notification) implements AgentComponentResult {
        @Override public String operation() { return "mcp_transport_try_notification"; }
        @Override public JsonNode value() { return nullableObject("notification", notification); }
    }
    record Embedded(List<Double> vector) implements AgentComponentResult {
        public Embedded {
            if (vector == null || vector.stream().anyMatch(value -> value == null || !Double.isFinite(value))) {
                throw new IllegalArgumentException("embedding vector must contain finite numbers");
            }
        }
        @Override public String operation() { return "embedder_embed"; }
        @Override public JsonNode value() {
            var object = JsonSupport.MAPPER.createObjectNode();
            var array = object.putArray("vector");
            vector.forEach(array::add);
            return object;
        }
    }
    record MemoryPromoted(BigInteger submitted, BigInteger promoted, BigInteger deduplicated)
            implements AgentComponentResult {
        @Override public String operation() { return "memory_promoter_promote"; }
        @Override public JsonNode value() {
            return JsonSupport.MAPPER.createObjectNode()
                    .put("submitted", TypedExtensionSupport.canonicalU64(submitted.toString(), "submitted"))
                    .put("promoted", TypedExtensionSupport.canonicalU64(promoted.toString(), "promoted"))
                    .put("deduplicated", TypedExtensionSupport.canonicalU64(
                            deduplicated.toString(), "deduplicated"));
        }
    }
    record WorkflowCompleted(JsonNode output) implements AgentComponentResult {
        @Override public String operation() { return "workflow_run"; }
        @Override public JsonNode value() { return object("output", output); }
    }
    record IntentClassified(JsonNode intent) implements AgentComponentResult {
        @Override public String operation() { return "intent_classify"; }
        @Override public JsonNode value() { return object("intent", intent); }
    }
    record SkillLoadAllowed(boolean allowed) implements AgentComponentResult {
        @Override public String operation() { return "skill_load_allows"; }
        @Override public JsonNode value() {
            return JsonSupport.MAPPER.createObjectNode().put("allowed", allowed);
        }
    }

    default String component() {
        var operation = operation();
        if (operation.startsWith("conversation_")) return "conversation_store";
        if (operation.startsWith("run_")) return "run_store";
        if (operation.startsWith("runtime_")) return "runtime_state_store";
        if (operation.startsWith("audit_")) return "audit_logger";
        if (operation.startsWith("workflow_checkpoint_")) return "workflow_checkpoint_store";
        if (operation.startsWith("revisioned_task_")) return "revisioned_task_store";
        if (operation.startsWith("sandbox_")) return "sandbox_executor";
        if (operation.startsWith("mcp_transport_")) return "mcp_transport";
        return switch (operation) {
            case "context_project" -> "context_projector";
            case "memory_trigger" -> "memory_trigger_sink";
            case "guard_check" -> "guard";
            case "search_provider_search" -> "search_provider";
            case "embedder_embed" -> "embedder";
            case "memory_promoter_promote" -> "memory_promoter";
            case "workflow_run", "workflow_run_stream" -> "workflow";
            case "intent_classify" -> "intent_classifier";
            case "skill_load_allows" -> "skill_load_policy";
            default -> throw new IllegalStateException("unknown Agent component result operation");
        };
    }

    private static ObjectNode object(String field, JsonNode value) {
        if (value == null) throw new IllegalArgumentException(field + " must not be null");
        return JsonSupport.MAPPER.createObjectNode().set(field, value.deepCopy());
    }
    private static ObjectNode nullableObject(String field, JsonNode value) {
        var object = JsonSupport.MAPPER.createObjectNode();
        if (value == null) object.putNull(field); else object.set(field, value.deepCopy());
        return object;
    }
    private static ObjectNode array(String field, List<JsonNode> values) {
        if (values == null) throw new IllegalArgumentException(field + " must not be null");
        var object = JsonSupport.MAPPER.createObjectNode();
        var array = object.putArray(field);
        values.forEach(value -> array.add(value.deepCopy()));
        return object;
    }
}
