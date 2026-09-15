export type JsonValue =
  | null
  | boolean
  | string
  | number
  | JsonValue[]
  | { readonly [key: string]: JsonValue };

export type WireValue =
  | { readonly kind: "null" }
  | { readonly kind: "bool"; readonly value: boolean }
  | { readonly kind: "string"; readonly value: string }
  | { readonly kind: "i64" | "u64" | "f64"; readonly value: string | number }
  | { readonly kind: "bytes"; readonly value: { readonly base64: string } }
  | { readonly kind: "duration"; readonly value: { readonly seconds: string; readonly nanos: number } }
  | { readonly kind: "timestamp"; readonly value: { readonly unix_seconds: string; readonly nanos: number; readonly rfc3339?: string } }
  | { readonly kind: "path"; readonly value: unknown }
  | { readonly kind: "handle"; readonly value: WireHandle }
  | { readonly kind: "list"; readonly value: WireValue[] }
  | { readonly kind: "map"; readonly value: Array<{ key: WireValue; value: WireValue }> }
  | { readonly kind: "record" | "variant" | "unknown"; readonly value: unknown };

/** Result of the framework-owned TurnOutcome classifier; Null means non-terminal. */
export type TurnOutcomeClassification = WireValue | null;

export interface WireHandle {
  readonly id: string;
  readonly generation: string;
  readonly kind: string;
}

export type WireU64 = string;
export type WireDuration = { readonly seconds: WireU64; readonly nanos: number };

export type WirePath =
  | { readonly encoding: "utf8"; readonly path: string }
  | { readonly encoding: "unix"; readonly bytes_base64: string; readonly display?: string | null }
  | { readonly encoding: "windows"; readonly utf16_base64: string; readonly display?: string | null };

export type ExtensionKind =
  | "tool"
  | "llm_client"
  | "store"
  | "critic"
  | "human_loop_provider"
  | "hook"
  | "agent_callback"
  | "intervention_callback"
  | "agent_factory"
  | "custom_agent"
  | "channel_plugin"
  | "channel_message_handler"
  | "context_compressor"
  | "agent_component";

/** Versioned descriptor accepted by the Host's typed extension registry. */
export type ExtensionDescriptor = {
  readonly kind: ExtensionKind;
  readonly descriptor_version: number;
  readonly [key: string]: unknown;
};

export type ToolPermission = "read" | "write" | "network" | "execute" | "sensitive";
export type ToolRiskLevel = "read_only" | "standard" | "dangerous";
export type SearchMode = "keyword" | "semantic" | "hybrid";

export interface ToolDescriptor extends ExtensionDescriptor {
  readonly kind: "tool";
  readonly descriptor_version: 1;
  readonly name: string;
  readonly description: string;
  readonly parameters: WireValue;
  readonly schema_revision: WireU64;
  readonly supports_streaming: boolean;
  readonly allows_parallel_batch_execution?: boolean;
  readonly exempt_from_batch_timeout?: boolean;
  readonly manages_own_timeout?: boolean;
  readonly required_input_modalities?: readonly string[];
  readonly required_permissions?: readonly ToolPermission[];
  readonly risk_level?: ToolRiskLevel;
}

export interface LlmCapabilities {
  readonly streaming_tool_calls: boolean;
  readonly named_sse_events: boolean;
  readonly reasoning_content: boolean;
  readonly image_input: boolean;
  readonly system_as_top_level: boolean;
  readonly ndjson_streaming: boolean;
  readonly tool_support: boolean;
  readonly structured_output: boolean;
  readonly requires_version_header: boolean;
  readonly supports_parallel_tool_calls: boolean;
  readonly supports_tool_choice_none: boolean;
  readonly tokenizer_name?: string | null;
}

export interface LlmClientDescriptor extends ExtensionDescriptor {
  readonly kind: "llm_client";
  readonly descriptor_version: 1;
  readonly model_name: string;
  readonly supports_streaming: boolean;
  readonly capabilities?: LlmCapabilities;
}

export interface StoreDescriptor extends ExtensionDescriptor {
  readonly kind: "store";
  readonly descriptor_version: 1;
  readonly search_modes?: readonly SearchMode[];
}

export interface CriticDescriptor extends ExtensionDescriptor {
  readonly kind: "critic";
  readonly descriptor_version: 1;
  readonly name: string;
}

export interface ToolOutputArtifactConfig {
  readonly retention: string;
  readonly root_dir: WirePath;
  readonly threshold_bytes: WireU64;
  readonly max_age_secs?: WireU64 | null;
}

export interface ToolContext {
  readonly active_message?: LlmMessage | null;
  readonly call_id?: string | null;
  readonly conversation_id?: string | null;
  readonly execution_id?: string | null;
  readonly message_id?: string | null;
  readonly output_artifacts?: ToolOutputArtifactConfig | null;
  readonly run_id?: string | null;
  readonly turn_id?: string | null;
  readonly working_dir?: WirePath | null;
}

export interface ToolExecuteInput {
  readonly parameters: WireValue;
  readonly context?: ToolContext | null;
}

export interface ToolValidateInput {
  readonly parameters: WireValue;
}

export type ToolResultKind =
  | { readonly kind: "text" }
  | { readonly kind: "json" }
  | { readonly kind: "image"; readonly mime_type: string }
  | { readonly kind: "table"; readonly columns: readonly string[]; readonly rows: readonly (readonly string[])[] }
  | { readonly kind: "diff"; readonly unified_diff: string }
  | { readonly kind: "file_reference"; readonly path: string }
  | { readonly kind: "command_output"; readonly exit_code?: number | null }
  | { readonly kind: "skill_activation"; readonly name: string }
  | { readonly kind: "structured_error"; readonly error_code: string };

export type ToolFailureCategory =
  | "invalid_arguments"
  | "unavailable"
  | "timeout"
  | "cancelled"
  | "transient"
  | "permanent"
  | "partial_side_effect";

export type ToolRecoveryAction =
  | "correct_arguments"
  | "retry"
  | "restore_then_retry"
  | "verify_then_retry"
  | "stop";

export type ToolSideEffect = "none" | "possible" | "confirmed";

export interface ToolFailure {
  readonly category: ToolFailureCategory;
  readonly recovery: ToolRecoveryAction;
  readonly side_effect: ToolSideEffect;
  readonly idempotency_key?: string | null;
  readonly postcondition?: string | null;
  readonly retry_after_ms?: WireU64 | null;
}

export interface ToolOutputArtifactRef {
  readonly artifact_bytes: WireU64;
  readonly path: WirePath;
  readonly payload_bytes: WireU64;
  readonly retention: string;
  readonly sha256: string;
}

export interface ToolResultContent {
  readonly kind: "image_url";
  readonly url: string;
  readonly detail?: string | null;
}

export interface ToolResultData {
  readonly kind: ToolResultKind;
  readonly output: string;
  readonly success: boolean;
  readonly truncated: boolean;
  readonly artifact?: ToolOutputArtifactRef | null;
  readonly data?: WireValue | null;
  readonly error?: string | null;
  readonly failure?: ToolFailure | null;
  readonly metadata?: Readonly<Record<string, string>>;
  readonly mime_type?: string | null;
  readonly model_content?: readonly ToolResultContent[];
}

/** Structural ToolResult type; the runtime factory is exported from helpers. */
export type ToolResult = ToolResultData;

export type LlmReasoningBlock =
  | { readonly kind: "signed"; readonly signature: string; readonly thinking: string }
  | { readonly kind: "redacted"; readonly data: string }
  | { readonly kind: "opaque"; readonly data: string; readonly id: string; readonly provider: string; readonly summary: readonly string[] };

export interface LlmToolCall {
  readonly arguments: string;
  readonly call_type: string;
  readonly function_name: string;
  readonly id: string;
}

export interface LlmMessage {
  readonly content: WireValue;
  readonly role: string;
  readonly name?: string | null;
  readonly reasoning_blocks?: readonly LlmReasoningBlock[] | null;
  readonly reasoning_content?: string | null;
  readonly tool_call_id?: string | null;
  readonly tool_calls?: readonly LlmToolCall[] | null;
}

export interface LlmToolDefinition {
  readonly description: string;
  readonly name: string;
  readonly parameters: WireValue;
  readonly tool_type: string;
}

export interface LlmChatRequest {
  readonly messages: readonly LlmMessage[];
  readonly cache_hints?: WireValue | null;
  readonly max_tokens?: number | null;
  readonly response_format?: WireValue | null;
  readonly temperature?: number | null;
  readonly thinking?: WireValue | null;
  readonly timeouts?: WireValue | null;
  readonly tool_choice?: string | null;
  readonly tools?: readonly LlmToolDefinition[] | null;
  readonly user_id?: string | null;
}

export interface LlmUsage {
  readonly value: WireValue;
}

export interface LlmChatResponse {
  readonly message: LlmMessage;
  readonly raw: WireValue;
  readonly finish_reason?: string | null;
  readonly usage?: LlmUsage | null;
}

export interface StorePutInput {
  readonly key: string;
  readonly namespace: readonly string[];
  readonly value: WireValue;
}

export interface StoreKeyInput {
  readonly key: string;
  readonly namespace: readonly string[];
}

export interface StoreSearchInput {
  readonly limit: WireU64;
  readonly namespace: readonly string[];
  readonly query: string;
}

export interface StoreSearchQuery {
  readonly limit: WireU64;
  readonly mode: { readonly kind: SearchMode; readonly vector_weight?: number | null };
  readonly text: string;
}

export interface StoreSearchWithInput {
  readonly namespace: readonly string[];
  readonly query: StoreSearchQuery;
}

export interface StoreListNamespacesInput {
  readonly prefix?: readonly string[] | null;
}

export interface StoreNamespaceInput {
  readonly namespace: readonly string[];
}

export interface StoreItem {
  readonly created_at: WireU64;
  readonly importance: number;
  readonly key: string;
  readonly namespace: readonly string[];
  readonly updated_at: WireU64;
  readonly value: WireValue;
  readonly expires_at?: WireU64 | null;
  readonly last_accessed?: WireU64 | null;
  readonly score?: number | null;
}

export type ToolInvocation =
  | { readonly operation: "tool_execute"; readonly input: ToolExecuteInput }
  | { readonly operation: "tool_execute_stream"; readonly input: ToolExecuteInput }
  | { readonly operation: "tool_validate_parameters"; readonly input: ToolValidateInput };

export type LlmInvocation =
  | { readonly operation: "llm_chat"; readonly input: LlmChatRequest }
  | { readonly operation: "llm_chat_stream"; readonly input: LlmChatRequest };

export type StoreInvocation =
  | { readonly operation: "store_put"; readonly input: StorePutInput }
  | { readonly operation: "store_get"; readonly input: StoreKeyInput }
  | { readonly operation: "store_search"; readonly input: StoreSearchInput }
  | { readonly operation: "store_search_with"; readonly input: StoreSearchWithInput }
  | { readonly operation: "store_delete"; readonly input: StoreKeyInput }
  | { readonly operation: "store_list_namespaces"; readonly input: StoreListNamespacesInput }
  | { readonly operation: "store_list"; readonly input: StoreNamespaceInput }
  | { readonly operation: "store_prune_expired"; readonly input: StoreNamespaceInput }
  | { readonly operation: "store_dedup_by_content"; readonly input: StoreNamespaceInput };

export interface CritiqueInput {
  readonly task: string;
  readonly answer: string;
  readonly context: string;
}

export type CriticInvocation = { readonly operation: "critic_critique"; readonly input: CritiqueInput };

export interface CompressionInput {
  readonly messages: readonly LlmMessage[];
  readonly token_limit: WireU64;
  readonly current_query?: string | null;
  readonly focus_instructions?: string | null;
  readonly tokenizer: TokenizerReference;
}

export interface TokenizerReference {
  readonly resource: WireHandle;
  readonly owner_session_id: string;
}

export interface ContextCompressorDescriptor {
  readonly kind: "context_compressor";
  readonly descriptor_version: 1;
  readonly name: string;
}

export type AgentComponentKind =
  | "conversation_store"
  | "run_store"
  | "runtime_state_store"
  | "audit_logger"
  | "context_projector"
  | "memory_trigger_sink"
  | "guard"
  | "search_provider"
  | "workflow_checkpoint_store"
  | "revisioned_task_store"
  | "sandbox_executor"
  | "mcp_transport"
  | "embedder"
  | "memory_promoter"
  | "workflow"
  | "intent_classifier"
  | "skill_load_policy";

export type AgentComponentOperation =
  | "conversation_create"
  | "conversation_get"
  | "conversation_list"
  | "conversation_update"
  | "conversation_delete"
  | "conversation_save_messages"
  | "conversation_get_messages"
  | "conversation_count_messages"
  | "conversation_ensure"
  | "conversation_search"
  | "run_save"
  | "run_load"
  | "run_list_by_session"
  | "run_list_all"
  | "run_append_event"
  | "run_list_by_parent"
  | "runtime_get_checkpoint"
  | "runtime_save_checkpoint"
  | "runtime_save_checkpoint_for_scope"
  | "runtime_state_ids"
  | "runtime_clear_state"
  | "runtime_clear_scope"
  | "runtime_clear_conversation"
  | "audit_log"
  | "audit_query"
  | "context_project"
  | "memory_trigger"
  | "guard_check"
  | "search_provider_search"
  | "workflow_checkpoint_save"
  | "workflow_checkpoint_save_if_generation"
  | "workflow_checkpoint_load"
  | "workflow_checkpoint_claim"
  | "workflow_checkpoint_ack_claim"
  | "workflow_checkpoint_requeue_claim"
  | "workflow_checkpoint_renew_claim"
  | "workflow_checkpoint_list"
  | "workflow_checkpoint_list_by_graph"
  | "workflow_checkpoint_list_filtered"
  | "workflow_checkpoint_delete"
  | "workflow_checkpoint_clear"
  | "revisioned_task_load"
  | "revisioned_task_compare_and_commit"
  | "sandbox_is_available"
  | "sandbox_execute"
  | "sandbox_execute_stream"
  | "sandbox_execute_with_limits"
  | "sandbox_execute_with_limits_and_cancel"
  | "sandbox_cleanup"
  | "mcp_transport_send"
  | "mcp_transport_notify"
  | "mcp_transport_close"
  | "mcp_transport_try_notification"
  | "embedder_embed"
  | "memory_promoter_promote"
  | "workflow_run"
  | "workflow_run_stream"
  | "intent_classify"
  | "skill_load_allows";

export interface SkillDescriptorPolicy {
  readonly name: string;
  readonly description: string;
  readonly location: WirePath;
  readonly license?: string | null;
  readonly compatibility?: string | null;
  readonly metadata: Readonly<Record<string, string>>;
  readonly source?: string | null;
  readonly allowed_tools: readonly string[];
  readonly shell?: string | null;
  readonly paths: readonly string[];
  readonly triggers: readonly string[];
  readonly hooks?: WireValue | null;
  readonly sandbox?: WireValue | null;
  readonly depends_on: readonly string[];
}

export interface AgentComponentDescriptor {
  readonly kind: "agent_component";
  readonly descriptor_version: 1;
  readonly component: AgentComponentKind;
  readonly name: string;
  readonly capabilities?: {
    readonly isolation_level?: "none" | "process" | "os-sandbox" | "container" | "orchestrated" | null;
    readonly supports_streaming?: boolean;
    readonly supports_notifications?: boolean;
    readonly claim_heartbeat_interval_ms?: WireU64 | null;
  };
}

export type AgentComponentCall =
  | { readonly operation: "conversation_create"; readonly input: { readonly conversation: WireValue } }
  | { readonly operation: "conversation_get"; readonly input: { readonly conversation_id: string } }
  | { readonly operation: "conversation_list"; readonly input: { readonly user_id?: string | null; readonly agent_type?: string | null; readonly limit?: WireU64 | null; readonly offset?: WireU64 | null } }
  | { readonly operation: "conversation_update"; readonly input: { readonly conversation_id: string; readonly title?: string | null; readonly summary?: string | null; readonly compressed_before_id?: string | null } }
  | { readonly operation: "conversation_delete"; readonly input: { readonly conversation_id: string } }
  | { readonly operation: "conversation_save_messages"; readonly input: { readonly conversation_id: string; readonly messages: readonly WireValue[] } }
  | { readonly operation: "conversation_get_messages"; readonly input: { readonly conversation_id: string } }
  | { readonly operation: "conversation_count_messages"; readonly input: { readonly conversation_id: string } }
  | { readonly operation: "conversation_ensure"; readonly input: { readonly conversation: WireValue } }
  | { readonly operation: "conversation_search"; readonly input: { readonly query: string; readonly limit: WireU64 } }
  | { readonly operation: "run_save"; readonly input: { readonly run: WireValue } }
  | { readonly operation: "run_load"; readonly input: { readonly run_id: string } }
  | { readonly operation: "run_list_by_session"; readonly input: { readonly session_id: string } }
  | { readonly operation: "run_list_all"; readonly input: { readonly limit: WireU64 } }
  | { readonly operation: "run_append_event"; readonly input: { readonly run_id: string; readonly event: WireValue } }
  | { readonly operation: "run_list_by_parent"; readonly input: { readonly parent_run_id: string } }
  | { readonly operation: "runtime_get_checkpoint"; readonly input: { readonly conversation_id: string } }
  | { readonly operation: "runtime_save_checkpoint"; readonly input: { readonly checkpoint: WireValue } }
  | { readonly operation: "runtime_save_checkpoint_for_scope"; readonly input: { readonly scope_id: string; readonly checkpoint: WireValue } }
  | { readonly operation: "runtime_state_ids"; readonly input: { readonly scope_id: string } }
  | { readonly operation: "runtime_clear_state"; readonly input: { readonly scope_id: string; readonly runtime_state_id: string } }
  | { readonly operation: "runtime_clear_scope"; readonly input: { readonly scope_id: string } }
  | { readonly operation: "runtime_clear_conversation"; readonly input: { readonly conversation_id: string } }
  | { readonly operation: "audit_log"; readonly input: { readonly event: WireValue } }
  | { readonly operation: "audit_query"; readonly input: { readonly session_id?: string | null; readonly agent_name?: string | null; readonly from?: string | null; readonly to?: string | null; readonly limit?: WireU64 | null } }
  | { readonly operation: "context_project"; readonly input: { readonly iteration: WireU64; readonly agent_name: string; readonly session_id?: string | null; readonly conversation_id?: string | null; readonly run_id?: string | null; readonly turn_id?: string | null } }
  | { readonly operation: "memory_trigger"; readonly input: { readonly trigger: WireValue } }
  | { readonly operation: "guard_check"; readonly input: { readonly content: string; readonly direction: "input" | "output" | "tool_input" | "tool_output" } }
  | { readonly operation: "search_provider_search"; readonly input: { readonly query: string; readonly max_results: WireU64 } }
  | { readonly operation: "workflow_checkpoint_save"; readonly input: { readonly checkpoint: WireValue } }
  | { readonly operation: "workflow_checkpoint_save_if_generation"; readonly input: { readonly checkpoint: WireValue; readonly expected_generation: WireU64 } }
  | { readonly operation: "workflow_checkpoint_load" | "workflow_checkpoint_claim" | "workflow_checkpoint_delete"; readonly input: { readonly checkpoint_id: string } }
  | { readonly operation: "workflow_checkpoint_ack_claim" | "workflow_checkpoint_requeue_claim" | "workflow_checkpoint_renew_claim"; readonly input: { readonly checkpoint_id: string; readonly attempt_id: string } }
  | { readonly operation: "workflow_checkpoint_list" | "workflow_checkpoint_clear"; readonly input: Record<string, never> }
  | { readonly operation: "workflow_checkpoint_list_by_graph"; readonly input: { readonly graph_name: string } }
  | { readonly operation: "workflow_checkpoint_list_filtered"; readonly input: { readonly filter: WireValue } }
  | { readonly operation: "revisioned_task_load"; readonly input: { readonly scope_id: string } }
  | { readonly operation: "revisioned_task_compare_and_commit"; readonly input: { readonly scope_id: string; readonly commit: WireValue } }
  | { readonly operation: "sandbox_is_available" | "sandbox_cleanup" | "mcp_transport_close" | "mcp_transport_try_notification"; readonly input: Record<string, never> }
  | { readonly operation: "sandbox_execute"; readonly input: { readonly command: WireValue } }
  | { readonly operation: "sandbox_execute_stream"; readonly input: { readonly command: WireValue } }
  | { readonly operation: "sandbox_execute_with_limits"; readonly input: { readonly command: WireValue; readonly limits: WireValue } }
  | { readonly operation: "sandbox_execute_with_limits_and_cancel"; readonly input: { readonly command: WireValue; readonly limits: WireValue } }
  | { readonly operation: "mcp_transport_send"; readonly input: { readonly request: WireValue } }
  | { readonly operation: "mcp_transport_notify"; readonly input: { readonly notification: WireValue } }
  | { readonly operation: "embedder_embed"; readonly input: { readonly text: string } }
  | { readonly operation: "memory_promoter_promote"; readonly input: { readonly evicted: readonly LlmMessage[] } }
  | { readonly operation: "workflow_run"; readonly input: { readonly input: string } }
  | { readonly operation: "workflow_run_stream"; readonly input: { readonly input: string } }
  | { readonly operation: "intent_classify"; readonly input: { readonly user_input: string; readonly context: readonly LlmMessage[] } }
  | { readonly operation: "skill_load_allows"; readonly input: { readonly descriptor: SkillDescriptorPolicy } };

export type AgentComponentCallResult =
  | { readonly operation: "conversation_create"; readonly value: { readonly conversation: WireValue } }
  | { readonly operation: "conversation_get"; readonly value: { readonly conversation?: WireValue | null } }
  | { readonly operation: "conversation_list"; readonly value: { readonly conversations: readonly WireValue[] } }
  | { readonly operation: "conversation_update" }
  | { readonly operation: "conversation_delete" }
  | { readonly operation: "conversation_save_messages" }
  | { readonly operation: "conversation_get_messages"; readonly value: { readonly messages: readonly WireValue[] } }
  | { readonly operation: "conversation_count_messages"; readonly value: { readonly count: WireU64 } }
  | { readonly operation: "conversation_ensure"; readonly value: { readonly conversation: WireValue } }
  | { readonly operation: "conversation_search"; readonly value: { readonly conversations: readonly WireValue[] } }
  | { readonly operation: "run_save" }
  | { readonly operation: "run_load"; readonly value: { readonly run?: WireValue | null } }
  | { readonly operation: "run_list_by_session"; readonly value: { readonly runs: readonly WireValue[] } }
  | { readonly operation: "run_list_all"; readonly value: { readonly runs: readonly WireValue[] } }
  | { readonly operation: "run_append_event" }
  | { readonly operation: "run_list_by_parent"; readonly value: { readonly runs: readonly WireValue[] } }
  | { readonly operation: "runtime_get_checkpoint"; readonly value: { readonly checkpoint?: WireValue | null } }
  | { readonly operation: "runtime_save_checkpoint" }
  | { readonly operation: "runtime_save_checkpoint_for_scope" }
  | { readonly operation: "runtime_state_ids"; readonly value: { readonly state_ids: readonly string[] } }
  | { readonly operation: "runtime_clear_state"; readonly value: { readonly receipt: WireValue } }
  | { readonly operation: "runtime_clear_scope"; readonly value: { readonly receipt: WireValue } }
  | { readonly operation: "runtime_clear_conversation" }
  | { readonly operation: "audit_log" }
  | { readonly operation: "audit_query"; readonly value: { readonly events: readonly WireValue[] } }
  | { readonly operation: "context_project"; readonly value: { readonly projections: readonly WireValue[] } }
  | { readonly operation: "memory_trigger"; readonly value: { readonly disposition: "persist" | "captured" } }
  | { readonly operation: "guard_check"; readonly value: { readonly result: WireValue } }
  | { readonly operation: "search_provider_search"; readonly value: { readonly results: readonly WireValue[] } }
  | { readonly operation: "workflow_checkpoint_save" | "workflow_checkpoint_delete" | "workflow_checkpoint_clear" | "sandbox_cleanup" | "mcp_transport_notify" | "mcp_transport_close" }
  | { readonly operation: "workflow_checkpoint_ack_claim" | "workflow_checkpoint_requeue_claim" | "workflow_checkpoint_renew_claim" }
  | { readonly operation: "workflow_checkpoint_save_if_generation"; readonly value: { readonly committed: boolean } }
  | { readonly operation: "workflow_checkpoint_load" | "workflow_checkpoint_claim"; readonly value: { readonly checkpoint?: WireValue | null } }
  | { readonly operation: "workflow_checkpoint_list" | "workflow_checkpoint_list_by_graph" | "workflow_checkpoint_list_filtered"; readonly value: { readonly checkpoints: readonly WireValue[] } }
  | { readonly operation: "revisioned_task_load"; readonly value: { readonly graph?: WireValue | null } }
  | { readonly operation: "revisioned_task_compare_and_commit"; readonly value: { readonly graph: WireValue } }
  | { readonly operation: "sandbox_is_available"; readonly value: { readonly available: boolean } }
  | { readonly operation: "sandbox_execute" | "sandbox_execute_with_limits"; readonly value: { readonly result: WireValue } }
  | { readonly operation: "sandbox_execute_with_limits_and_cancel"; readonly value: { readonly result: WireValue } }
  | { readonly operation: "mcp_transport_send"; readonly value: { readonly response: WireValue } }
  | { readonly operation: "mcp_transport_try_notification"; readonly value: { readonly notification?: WireValue | null } }
  | { readonly operation: "embedder_embed"; readonly value: { readonly vector: readonly number[] } }
  | { readonly operation: "memory_promoter_promote"; readonly value: { readonly submitted: WireU64; readonly promoted: WireU64; readonly deduplicated: WireU64 } }
  | { readonly operation: "workflow_run"; readonly value: { readonly output: WireValue } }
  | { readonly operation: "intent_classify"; readonly value: { readonly intent: WireValue } }
  | { readonly operation: "skill_load_allows"; readonly value: { readonly allowed: boolean } };

export type AgentComponentInvocation = {
  readonly operation: "agent_component_call" | "agent_component_call_stream";
  readonly input: {
    readonly component: AgentComponentKind;
    readonly call: AgentComponentCall;
  };
};

export type AgentComponentStreamChunk =
  | { readonly component: "sandbox"; readonly event: { readonly event: "output"; readonly channel: "stdout" | "stderr"; readonly chunk: string } }
  | { readonly component: "workflow"; readonly event:
      | { readonly event: "node_start"; readonly node_name: string; readonly step_index: WireU64 }
      | { readonly event: "node_end"; readonly node_name: string; readonly step_index: WireU64; readonly elapsed: WireDuration }
      | { readonly event: "token"; readonly node_name: string; readonly token: string }
      | { readonly event: "node_error"; readonly node_name: string; readonly error: string }
    };

export type AgentComponentStreamComplete =
  | { readonly component: "sandbox"; readonly terminal:
      | { readonly terminal: "complete"; readonly result: WireValue }
      | { readonly terminal: "failed"; readonly failure:
          | { readonly kind: "cancelled"; readonly message: string }
          | { readonly kind: "io_error"; readonly message: string }
        }
    }
  | { readonly component: "workflow"; readonly terminal: {
      readonly result: string;
      readonly total_steps: WireU64;
      readonly elapsed: WireDuration;
    } };

export type AgentComponentStreamChunkValue = {
  readonly kind: "agent_component";
  readonly value: AgentComponentStreamChunk;
};

export type AgentComponentStreamCompleteValue = {
  readonly kind: "agent_component";
  readonly value: AgentComponentStreamComplete;
};

export interface CompressionOutput {
  readonly messages: readonly LlmMessage[];
  readonly evicted: readonly LlmMessage[];
  readonly checkpoint?: WireValue | null;
}

export type ContextCompressorInvocation = {
  readonly operation: "compressor_compress";
  readonly input: CompressionInput;
};

export type ExtensionInvocation = ToolInvocation | LlmInvocation | StoreInvocation | CriticInvocation | ContextCompressorInvocation | AgentComponentInvocation;

export interface ExtensionInvocationContext {
  readonly call_id?: string | null;
  readonly execution_id?: string | null;
  readonly message_id?: string | null;
  readonly run_id?: string | null;
  readonly session_id?: string | null;
  readonly stream_id?: string | null;
  readonly turn_id?: string | null;
}

export interface ExtensionInvokeCall<I extends ExtensionInvocation = ExtensionInvocation> {
  readonly context?: ExtensionInvocationContext | null;
  readonly deadline: { readonly seconds: string; readonly nanos: number };
  readonly extension: WireHandle;
  readonly invocation: I;
  readonly invocation_id: string;
  readonly stream?: WireHandle | null;
}

export type ToolExtensionResult =
  | { readonly operation: "tool_execute"; readonly value: ToolResultData }
  | { readonly operation: "tool_validate_parameters"; readonly value: string | null };

export type LlmExtensionResult = { readonly operation: "llm_chat"; readonly value: LlmChatResponse };

export type StoreExtensionResult =
  | { readonly operation: "store_put"; readonly value: null }
  | { readonly operation: "store_get"; readonly value: StoreItem | null }
  | { readonly operation: "store_search"; readonly value: readonly StoreItem[] }
  | { readonly operation: "store_search_with"; readonly value: readonly StoreItem[] }
  | { readonly operation: "store_delete"; readonly value: boolean }
  | { readonly operation: "store_list_namespaces"; readonly value: readonly (readonly string[])[] }
  | { readonly operation: "store_list"; readonly value: readonly StoreItem[] }
  | { readonly operation: "store_prune_expired"; readonly value: WireU64 }
  | { readonly operation: "store_dedup_by_content"; readonly value: WireU64 };

export interface Critique {
  readonly score: number;
  readonly passed: boolean;
  readonly feedback: string;
  readonly suggestions: readonly string[];
}

/** Structured result returned by a Critic bridge invocation. */
export type CritiqueResult = Critique;

export type CriticExtensionResult = { readonly operation: "critic_critique"; readonly value: Critique };

export type ContextCompressorExtensionResult = {
  readonly operation: "compressor_compress";
  readonly value: CompressionOutput;
};

export type AgentComponentExtensionResult = {
  readonly operation: "agent_component_call";
  readonly value: {
    readonly component: AgentComponentKind;
    readonly result: AgentComponentCallResult;
  };
};

export type ExtensionResult = ToolExtensionResult | LlmExtensionResult | StoreExtensionResult | CriticExtensionResult | ContextCompressorExtensionResult | AgentComponentExtensionResult;

export type ExtensionOutcome<R = unknown> =
  | { readonly outcome: "result"; readonly result: R }
  | { readonly outcome: "stream"; readonly stream: WireHandle }
  | {
      readonly outcome: "error";
      readonly error: {
        readonly code: string;
        readonly message: string;
        readonly retryable: string;
        readonly operation?: string;
        readonly details?: unknown;
      };
    };

export type TypedExtensionHandler<I extends ExtensionInvocation, R extends ExtensionResult> = (
  call: ExtensionInvokeCall<I>,
  signal: AbortSignal,
) => Promise<R | ExtensionOutcome<R>> | R | ExtensionOutcome<R>;

export type ToolExtensionHandler = TypedExtensionHandler<ToolInvocation, ToolExtensionResult>;
export type LlmClientExtensionHandler = TypedExtensionHandler<LlmInvocation, LlmExtensionResult>;
export type StoreExtensionHandler = TypedExtensionHandler<StoreInvocation, StoreExtensionResult>;
export type CriticExtensionHandler = TypedExtensionHandler<CriticInvocation, CriticExtensionResult>;
export type ContextCompressorExtensionHandler = TypedExtensionHandler<ContextCompressorInvocation, ContextCompressorExtensionResult>;
export type AgentComponentExtensionHandler = TypedExtensionHandler<AgentComponentInvocation, AgentComponentExtensionResult>;

export interface EchoAgentCapability {
  readonly extension_protocol_version: number;
  readonly contract_digest: string;
  readonly source_contract_digest: string;
  readonly features: readonly string[];
  readonly capabilities: ReadonlyArray<{ capability: string; required: boolean }>;
  readonly limits: Record<string, unknown>;
}

export interface RunStartResult {
  readonly run: WireHandle;
  readonly stream: WireHandle;
  readonly first_event?: WireEventEnvelope;
}

/** Settled status reported by a TurnReceipt or TurnOutcome. */
export type TurnStatus = "completed" | "cancelled" | "failed";

/** Lossless usage facts reported by a settled turn receipt. */
export interface ExecutionUsage {
  readonly duration_ms: WireU64;
  readonly tokens_used: WireU64 | null;
  readonly iterations: WireU64 | null;
}

/** Idiomatic RunHandle name for the canonical execution usage shape. */
export type RunUsage = ExecutionUsage;

export interface RunGetResult {
  readonly status: string;
  readonly last_sequence: string;
  readonly stream?: WireHandle;
  readonly terminal?: unknown;
  readonly receipt?: unknown;
}

export interface RunWaitResult {
  readonly settled: boolean;
  readonly terminal?: unknown;
  readonly receipt?: unknown;
}

export interface RunCancelResult {
  readonly cancellation_initiated: boolean;
  readonly status: string;
}

export interface EventGap {
  readonly from_sequence: string;
  readonly to_sequence: string;
  readonly reason: string;
  readonly snapshot_watermark: string;
}

export interface WireEventEnvelope {
  readonly schema_version: number;
  readonly event_id: string;
  readonly content_hash: string;
  readonly sequence: string;
  readonly stream_id: string;
  readonly conversation_id?: string;
  readonly run_id?: string;
  readonly turn_id: string;
  readonly message_id?: string;
  readonly execution_id?: string;
  readonly parent_event_id?: string;
  readonly timestamp: {
    readonly unix_seconds: string;
    readonly nanos: number;
    readonly rfc3339?: string;
  };
  readonly payload: {
    readonly event_type: string;
    readonly data?: WireValue;
  };
}

export interface FacadeEvent {
  readonly stream: WireHandle;
  readonly envelope: WireEventEnvelope;
}

export interface FacadeGap {
  readonly stream: WireHandle;
  readonly gap: EventGap;
}

export type FacadeStreamItem = FacadeEvent | FacadeGap;

export interface ReplayResult {
  readonly requested_after_sequence: string;
  readonly events: readonly FacadeEvent[];
  readonly next_cursor: {
    readonly stream_id: string;
    readonly last_processed_sequence: string;
  };
  readonly gap?: FacadeGap;
}

export function isFacadeEvent(item: FacadeStreamItem): item is FacadeEvent {
  return "envelope" in item;
}

export function isFacadeGap(item: FacadeStreamItem): item is FacadeGap {
  return "gap" in item;
}
