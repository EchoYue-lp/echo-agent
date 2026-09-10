package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.math.BigInteger;

/** Closed, operation-discriminated input contract for Agent components. */
public sealed interface AgentComponentRequest permits
        AgentComponentRequest.ConversationCreate,
        AgentComponentRequest.ConversationGet,
        AgentComponentRequest.ConversationList,
        AgentComponentRequest.ConversationUpdate,
        AgentComponentRequest.ConversationDelete,
        AgentComponentRequest.ConversationSaveMessages,
        AgentComponentRequest.ConversationGetMessages,
        AgentComponentRequest.ConversationCountMessages,
        AgentComponentRequest.ConversationEnsure,
        AgentComponentRequest.ConversationSearch,
        AgentComponentRequest.RunSave,
        AgentComponentRequest.RunLoad,
        AgentComponentRequest.RunListBySession,
        AgentComponentRequest.RunListAll,
        AgentComponentRequest.RunAppendEvent,
        AgentComponentRequest.RunListByParent,
        AgentComponentRequest.RuntimeGetCheckpoint,
        AgentComponentRequest.RuntimeSaveCheckpoint,
        AgentComponentRequest.RuntimeSaveCheckpointForScope,
        AgentComponentRequest.RuntimeStateIds,
        AgentComponentRequest.RuntimeClearState,
        AgentComponentRequest.RuntimeClearScope,
        AgentComponentRequest.RuntimeClearConversation,
        AgentComponentRequest.AuditLog,
        AgentComponentRequest.AuditQuery,
        AgentComponentRequest.ContextProject,
        AgentComponentRequest.MemoryTrigger,
        AgentComponentRequest.GuardCheck,
        AgentComponentRequest.SearchProviderSearch,
        AgentComponentRequest.WorkflowCheckpointSave,
        AgentComponentRequest.WorkflowCheckpointLoad,
        AgentComponentRequest.WorkflowCheckpointClaim,
        AgentComponentRequest.WorkflowCheckpointList,
        AgentComponentRequest.WorkflowCheckpointListByGraph,
        AgentComponentRequest.WorkflowCheckpointListFiltered,
        AgentComponentRequest.WorkflowCheckpointDelete,
        AgentComponentRequest.WorkflowCheckpointClear,
        AgentComponentRequest.RevisionedTaskLoad,
        AgentComponentRequest.RevisionedTaskCompareAndCommit,
        AgentComponentRequest.SandboxIsAvailable,
        AgentComponentRequest.SandboxExecute,
        AgentComponentRequest.SandboxExecuteStream,
        AgentComponentRequest.SandboxExecuteWithLimits,
        AgentComponentRequest.SandboxExecuteWithLimitsAndCancel,
        AgentComponentRequest.SandboxCleanup,
        AgentComponentRequest.McpTransportSend,
        AgentComponentRequest.McpTransportNotify,
        AgentComponentRequest.McpTransportClose,
        AgentComponentRequest.McpTransportTryNotification,
        AgentComponentRequest.EmbedderEmbed,
        AgentComponentRequest.MemoryPromoterPromote,
        AgentComponentRequest.WorkflowRun,
        AgentComponentRequest.WorkflowRunStream,
        AgentComponentRequest.IntentClassify,
        AgentComponentRequest.SkillLoadAllows {

    String operation();

    record ConversationCreate(JsonNode conversation) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_create"; }
    }
    record ConversationGet(String conversationId) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_get"; }
    }
    record ConversationList(String userId, String agentType, BigInteger limit, BigInteger offset)
            implements AgentComponentRequest {
        @Override public String operation() { return "conversation_list"; }
    }
    record ConversationUpdate(String conversationId, String title, String summary, BigInteger compressedBeforeId)
            implements AgentComponentRequest {
        @Override public String operation() { return "conversation_update"; }
    }
    record ConversationDelete(String conversationId) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_delete"; }
    }
    record ConversationSaveMessages(String conversationId, JsonNode messages) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_save_messages"; }
    }
    record ConversationGetMessages(String conversationId) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_get_messages"; }
    }
    record ConversationCountMessages(String conversationId) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_count_messages"; }
    }
    record ConversationEnsure(JsonNode conversation) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_ensure"; }
    }
    record ConversationSearch(String query, BigInteger limit) implements AgentComponentRequest {
        @Override public String operation() { return "conversation_search"; }
    }
    record RunSave(JsonNode run) implements AgentComponentRequest {
        @Override public String operation() { return "run_save"; }
    }
    record RunLoad(String runId) implements AgentComponentRequest {
        @Override public String operation() { return "run_load"; }
    }
    record RunListBySession(String sessionId) implements AgentComponentRequest {
        @Override public String operation() { return "run_list_by_session"; }
    }
    record RunListAll(BigInteger limit) implements AgentComponentRequest {
        @Override public String operation() { return "run_list_all"; }
    }
    record RunAppendEvent(String runId, JsonNode event) implements AgentComponentRequest {
        @Override public String operation() { return "run_append_event"; }
    }
    record RunListByParent(String parentRunId) implements AgentComponentRequest {
        @Override public String operation() { return "run_list_by_parent"; }
    }
    record RuntimeGetCheckpoint(String conversationId) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_get_checkpoint"; }
    }
    record RuntimeSaveCheckpoint(JsonNode checkpoint) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_save_checkpoint"; }
    }
    record RuntimeSaveCheckpointForScope(String scopeId, JsonNode checkpoint) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_save_checkpoint_for_scope"; }
    }
    record RuntimeStateIds(String scopeId) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_state_ids"; }
    }
    record RuntimeClearState(String scopeId, String runtimeStateId) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_clear_state"; }
    }
    record RuntimeClearScope(String scopeId) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_clear_scope"; }
    }
    record RuntimeClearConversation(String conversationId) implements AgentComponentRequest {
        @Override public String operation() { return "runtime_clear_conversation"; }
    }
    record AuditLog(JsonNode event) implements AgentComponentRequest {
        @Override public String operation() { return "audit_log"; }
    }
    record AuditQuery(String sessionId, String agentName, String from, String to, BigInteger limit)
            implements AgentComponentRequest {
        @Override public String operation() { return "audit_query"; }
    }
    record ContextProject(BigInteger iteration, String agentName, String sessionId,
                          String conversationId, String runId, String turnId)
            implements AgentComponentRequest {
        @Override public String operation() { return "context_project"; }
    }
    record MemoryTrigger(JsonNode trigger) implements AgentComponentRequest {
        @Override public String operation() { return "memory_trigger"; }
    }
    record GuardCheck(String content, String direction) implements AgentComponentRequest {
        public GuardCheck {
            if (!java.util.Set.of("input", "output", "tool_input", "tool_output").contains(direction)) {
                throw new IllegalArgumentException("guard direction is invalid");
            }
        }
        @Override public String operation() { return "guard_check"; }
    }
    record SearchProviderSearch(String query, BigInteger maxResults) implements AgentComponentRequest {
        @Override public String operation() { return "search_provider_search"; }
    }
    record WorkflowCheckpointSave(JsonNode checkpoint) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_save"; }
    }
    record WorkflowCheckpointLoad(String checkpointId) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_load"; }
    }
    record WorkflowCheckpointClaim(String checkpointId) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_claim"; }
    }
    record WorkflowCheckpointList() implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_list"; }
    }
    record WorkflowCheckpointListByGraph(String graphName) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_list_by_graph"; }
    }
    record WorkflowCheckpointListFiltered(JsonNode filter) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_list_filtered"; }
    }
    record WorkflowCheckpointDelete(String checkpointId) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_delete"; }
    }
    record WorkflowCheckpointClear() implements AgentComponentRequest {
        @Override public String operation() { return "workflow_checkpoint_clear"; }
    }
    record RevisionedTaskLoad(String scopeId) implements AgentComponentRequest {
        @Override public String operation() { return "revisioned_task_load"; }
    }
    record RevisionedTaskCompareAndCommit(String scopeId, JsonNode commit) implements AgentComponentRequest {
        @Override public String operation() { return "revisioned_task_compare_and_commit"; }
    }
    record SandboxIsAvailable() implements AgentComponentRequest {
        @Override public String operation() { return "sandbox_is_available"; }
    }
    record SandboxExecute(JsonNode command) implements AgentComponentRequest {
        @Override public String operation() { return "sandbox_execute"; }
    }
    record SandboxExecuteStream(JsonNode command) implements AgentComponentRequest {
        @Override public String operation() { return "sandbox_execute_stream"; }
    }
    record SandboxExecuteWithLimits(JsonNode command, JsonNode limits) implements AgentComponentRequest {
        @Override public String operation() { return "sandbox_execute_with_limits"; }
    }
    record SandboxExecuteWithLimitsAndCancel(JsonNode command, JsonNode limits)
            implements AgentComponentRequest {
        @Override public String operation() { return "sandbox_execute_with_limits_and_cancel"; }
    }
    record SandboxCleanup() implements AgentComponentRequest {
        @Override public String operation() { return "sandbox_cleanup"; }
    }
    record McpTransportSend(JsonNode request) implements AgentComponentRequest {
        @Override public String operation() { return "mcp_transport_send"; }
    }
    record McpTransportNotify(JsonNode notification) implements AgentComponentRequest {
        @Override public String operation() { return "mcp_transport_notify"; }
    }
    record McpTransportClose() implements AgentComponentRequest {
        @Override public String operation() { return "mcp_transport_close"; }
    }
    record McpTransportTryNotification() implements AgentComponentRequest {
        @Override public String operation() { return "mcp_transport_try_notification"; }
    }
    record EmbedderEmbed(String text) implements AgentComponentRequest {
        @Override public String operation() { return "embedder_embed"; }
    }
    record MemoryPromoterPromote(JsonNode evicted) implements AgentComponentRequest {
        @Override public String operation() { return "memory_promoter_promote"; }
    }
    record WorkflowRun(String input) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_run"; }
    }
    record WorkflowRunStream(String input) implements AgentComponentRequest {
        @Override public String operation() { return "workflow_run_stream"; }
    }
    record IntentClassify(String userInput, JsonNode context) implements AgentComponentRequest {
        @Override public String operation() { return "intent_classify"; }
    }
    record SkillLoadAllows(JsonNode descriptor) implements AgentComponentRequest {
        @Override public String operation() { return "skill_load_allows"; }
    }

    static AgentComponentRequest from(JsonNode call) {
        if (call == null || !call.isObject() || !call.path("operation").isTextual()) {
            throw new IllegalArgumentException("AgentComponent call must contain operation and input");
        }
        var operation = call.path("operation").textValue();
        var allowed = allowedFields(operation);
        var input = call.get("input");
        if ((input == null || input.isNull()) && allowed.isEmpty()) {
            input = JsonSupport.MAPPER.createObjectNode();
        }
        if (input == null || !input.isObject()) {
            throw new IllegalArgumentException("AgentComponent call input must be an object");
        }
        input.fieldNames().forEachRemaining(field -> {
            if (!allowed.contains(field)) {
                throw new IllegalArgumentException(
                        "unknown AgentComponent input field for " + operation + ": " + field);
            }
        });
        return switch (operation) {
            case "conversation_create" -> new ConversationCreate(required(input, "conversation"));
            case "conversation_get" -> new ConversationGet(text(input, "conversation_id"));
            case "conversation_list" -> new ConversationList(optionalText(input, "user_id"),
                    optionalText(input, "agent_type"), optionalU64(input, "limit"), optionalU64(input, "offset"));
            case "conversation_update" -> new ConversationUpdate(text(input, "conversation_id"),
                    optionalText(input, "title"), optionalText(input, "summary"), optionalInteger(input, "compressed_before_id"));
            case "conversation_delete" -> new ConversationDelete(text(input, "conversation_id"));
            case "conversation_save_messages" -> new ConversationSaveMessages(
                    text(input, "conversation_id"), array(input, "messages"));
            case "conversation_get_messages" -> new ConversationGetMessages(text(input, "conversation_id"));
            case "conversation_count_messages" -> new ConversationCountMessages(text(input, "conversation_id"));
            case "conversation_ensure" -> new ConversationEnsure(required(input, "conversation"));
            case "conversation_search" -> new ConversationSearch(text(input, "query"), u64(input, "limit"));
            case "run_save" -> new RunSave(required(input, "run"));
            case "run_load" -> new RunLoad(text(input, "run_id"));
            case "run_list_by_session" -> new RunListBySession(text(input, "session_id"));
            case "run_list_all" -> new RunListAll(u64(input, "limit"));
            case "run_append_event" -> new RunAppendEvent(text(input, "run_id"), required(input, "event"));
            case "run_list_by_parent" -> new RunListByParent(text(input, "parent_run_id"));
            case "runtime_get_checkpoint" -> new RuntimeGetCheckpoint(text(input, "conversation_id"));
            case "runtime_save_checkpoint" -> new RuntimeSaveCheckpoint(required(input, "checkpoint"));
            case "runtime_save_checkpoint_for_scope" -> new RuntimeSaveCheckpointForScope(
                    text(input, "scope_id"), required(input, "checkpoint"));
            case "runtime_state_ids" -> new RuntimeStateIds(text(input, "scope_id"));
            case "runtime_clear_state" -> new RuntimeClearState(
                    text(input, "scope_id"), text(input, "runtime_state_id"));
            case "runtime_clear_scope" -> new RuntimeClearScope(text(input, "scope_id"));
            case "runtime_clear_conversation" -> new RuntimeClearConversation(text(input, "conversation_id"));
            case "audit_log" -> new AuditLog(required(input, "event"));
            case "audit_query" -> new AuditQuery(optionalText(input, "session_id"),
                    optionalText(input, "agent_name"), optionalText(input, "from"),
                    optionalText(input, "to"), optionalU64(input, "limit"));
            case "context_project" -> new ContextProject(u64(input, "iteration"),
                    text(input, "agent_name"), optionalText(input, "session_id"),
                    optionalText(input, "conversation_id"), optionalText(input, "run_id"),
                    optionalText(input, "turn_id"));
            case "memory_trigger" -> new MemoryTrigger(required(input, "trigger"));
            case "guard_check" -> new GuardCheck(text(input, "content"), text(input, "direction"));
            case "search_provider_search" -> new SearchProviderSearch(
                    text(input, "query"), u64(input, "max_results"));
            case "workflow_checkpoint_save" -> new WorkflowCheckpointSave(required(input, "checkpoint"));
            case "workflow_checkpoint_load" -> new WorkflowCheckpointLoad(text(input, "checkpoint_id"));
            case "workflow_checkpoint_claim" -> new WorkflowCheckpointClaim(text(input, "checkpoint_id"));
            case "workflow_checkpoint_list" -> new WorkflowCheckpointList();
            case "workflow_checkpoint_list_by_graph" -> new WorkflowCheckpointListByGraph(text(input, "graph_name"));
            case "workflow_checkpoint_list_filtered" -> new WorkflowCheckpointListFiltered(required(input, "filter"));
            case "workflow_checkpoint_delete" -> new WorkflowCheckpointDelete(text(input, "checkpoint_id"));
            case "workflow_checkpoint_clear" -> new WorkflowCheckpointClear();
            case "revisioned_task_load" -> new RevisionedTaskLoad(text(input, "scope_id"));
            case "revisioned_task_compare_and_commit" -> new RevisionedTaskCompareAndCommit(
                    text(input, "scope_id"), required(input, "commit"));
            case "sandbox_is_available" -> new SandboxIsAvailable();
            case "sandbox_execute" -> new SandboxExecute(required(input, "command"));
            case "sandbox_execute_stream" -> new SandboxExecuteStream(required(input, "command"));
            case "sandbox_execute_with_limits" -> new SandboxExecuteWithLimits(
                    required(input, "command"), required(input, "limits"));
            case "sandbox_execute_with_limits_and_cancel" -> new SandboxExecuteWithLimitsAndCancel(
                    required(input, "command"), required(input, "limits"));
            case "sandbox_cleanup" -> new SandboxCleanup();
            case "mcp_transport_send" -> new McpTransportSend(required(input, "request"));
            case "mcp_transport_notify" -> new McpTransportNotify(required(input, "notification"));
            case "mcp_transport_close" -> new McpTransportClose();
            case "mcp_transport_try_notification" -> new McpTransportTryNotification();
            case "embedder_embed" -> new EmbedderEmbed(text(input, "text"));
            case "memory_promoter_promote" -> new MemoryPromoterPromote(array(input, "evicted"));
            case "workflow_run" -> new WorkflowRun(text(input, "input"));
            case "workflow_run_stream" -> new WorkflowRunStream(text(input, "input"));
            case "intent_classify" -> new IntentClassify(text(input, "user_input"), array(input, "context"));
            case "skill_load_allows" -> new SkillLoadAllows(required(input, "descriptor"));
            default -> throw new IllegalArgumentException("unknown AgentComponent operation: " + operation);
        };
    }

    private static java.util.Set<String> allowedFields(String operation) {
        return switch (operation) {
            case "conversation_create" -> java.util.Set.of("conversation");
            case "conversation_get", "conversation_delete", "conversation_get_messages",
                    "conversation_count_messages", "runtime_get_checkpoint",
                    "runtime_clear_conversation" -> java.util.Set.of("conversation_id");
            case "conversation_list" -> java.util.Set.of("user_id", "agent_type", "limit", "offset");
            case "conversation_update" -> java.util.Set.of(
                    "conversation_id", "title", "summary", "compressed_before_id");
            case "conversation_save_messages" -> java.util.Set.of("conversation_id", "messages");
            case "conversation_ensure" -> java.util.Set.of("conversation");
            case "conversation_search" -> java.util.Set.of("query", "limit");
            case "run_save" -> java.util.Set.of("run");
            case "run_load" -> java.util.Set.of("run_id");
            case "run_list_by_session" -> java.util.Set.of("session_id");
            case "run_list_all" -> java.util.Set.of("limit");
            case "run_append_event" -> java.util.Set.of("run_id", "event");
            case "run_list_by_parent" -> java.util.Set.of("parent_run_id");
            case "runtime_save_checkpoint" -> java.util.Set.of("checkpoint");
            case "runtime_save_checkpoint_for_scope" -> java.util.Set.of("scope_id", "checkpoint");
            case "runtime_state_ids", "runtime_clear_scope", "revisioned_task_load" -> java.util.Set.of("scope_id");
            case "runtime_clear_state" -> java.util.Set.of("scope_id", "runtime_state_id");
            case "audit_log" -> java.util.Set.of("event");
            case "audit_query" -> java.util.Set.of("session_id", "agent_name", "from", "to", "limit");
            case "context_project" -> java.util.Set.of(
                    "iteration", "agent_name", "session_id", "conversation_id", "run_id", "turn_id");
            case "memory_trigger" -> java.util.Set.of("trigger");
            case "guard_check" -> java.util.Set.of("content", "direction");
            case "search_provider_search" -> java.util.Set.of("query", "max_results");
            case "workflow_checkpoint_save" -> java.util.Set.of("checkpoint");
            case "workflow_checkpoint_load", "workflow_checkpoint_claim",
                    "workflow_checkpoint_delete" -> java.util.Set.of("checkpoint_id");
            case "workflow_checkpoint_list", "workflow_checkpoint_clear",
                    "sandbox_is_available", "sandbox_cleanup", "mcp_transport_close",
                    "mcp_transport_try_notification" -> java.util.Set.of();
            case "workflow_checkpoint_list_by_graph" -> java.util.Set.of("graph_name");
            case "workflow_checkpoint_list_filtered" -> java.util.Set.of("filter");
            case "revisioned_task_compare_and_commit" -> java.util.Set.of("scope_id", "commit");
            case "sandbox_execute", "sandbox_execute_stream" -> java.util.Set.of("command");
            case "sandbox_execute_with_limits", "sandbox_execute_with_limits_and_cancel" ->
                    java.util.Set.of("command", "limits");
            case "mcp_transport_send" -> java.util.Set.of("request");
            case "mcp_transport_notify" -> java.util.Set.of("notification");
            case "embedder_embed", "workflow_run", "workflow_run_stream" -> java.util.Set.of(
                    "embedder_embed".equals(operation) ? "text" : "input");
            case "memory_promoter_promote" -> java.util.Set.of("evicted");
            case "intent_classify" -> java.util.Set.of("user_input", "context");
            case "skill_load_allows" -> java.util.Set.of("descriptor");
            default -> throw new IllegalArgumentException("unknown AgentComponent operation: " + operation);
        };
    }

    private static JsonNode required(JsonNode input, String field) {
        var value = input.get(field);
        if (value == null) throw new IllegalArgumentException(field + " is required");
        return value.deepCopy();
    }
    private static JsonNode array(JsonNode input, String field) {
        var value = required(input, field);
        if (!value.isArray()) throw new IllegalArgumentException(field + " must be an array");
        return value;
    }
    private static String text(JsonNode input, String field) {
        var value = input.get(field);
        if (value == null || !value.isTextual()) throw new IllegalArgumentException(field + " must be text");
        return value.textValue();
    }
    private static String optionalText(JsonNode input, String field) {
        var value = input.get(field);
        if (value == null || value.isNull()) return null;
        if (!value.isTextual()) throw new IllegalArgumentException(field + " must be text or null");
        return value.textValue();
    }
    private static BigInteger u64(JsonNode input, String field) {
        return new BigInteger(TypedExtensionSupport.canonicalU64(text(input, field), field));
    }
    private static BigInteger optionalU64(JsonNode input, String field) {
        var value = optionalText(input, field);
        return value == null ? null : new BigInteger(TypedExtensionSupport.canonicalU64(value, field));
    }
    private static BigInteger optionalInteger(JsonNode input, String field) {
        var value = optionalText(input, field);
        try {
            return value == null ? null : new BigInteger(value);
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException(field + " must be canonical integer text", error);
        }
    }
}
