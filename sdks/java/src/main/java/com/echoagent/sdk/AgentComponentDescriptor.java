package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Set;

/** Typed descriptor for a Host-consumed Agent infrastructure component. */
public final class AgentComponentDescriptor implements ExtensionDescriptor {
    private static final Set<String> COMPONENTS = Set.of(
            "conversation_store", "run_store", "runtime_state_store",
            "audit_logger", "context_projector", "memory_trigger_sink", "guard",
            "search_provider", "workflow_checkpoint_store", "revisioned_task_store",
            "sandbox_executor", "mcp_transport", "embedder", "memory_promoter", "workflow",
            "intent_classifier", "skill_load_policy");

    private final String component;
    private final String name;
    private final String isolationLevel;
    private final boolean supportsStreaming;
    private final boolean supportsNotifications;

    private AgentComponentDescriptor(
            String component, String name, String isolationLevel, boolean supportsStreaming,
            boolean supportsNotifications) {
        if (!COMPONENTS.contains(component)) {
            throw new IllegalArgumentException("unknown Agent component kind");
        }
        this.component = component;
        this.name = TypedExtensionSupport.requiredText(name, "name", 256);
        this.isolationLevel = isolationLevel;
        this.supportsStreaming = supportsStreaming;
        this.supportsNotifications = supportsNotifications;
        if (supportsStreaming && !Set.of("sandbox_executor", "workflow").contains(component)) {
            throw new IllegalArgumentException("streaming is only valid for sandbox and workflow");
        }
    }

    public static AgentComponentDescriptor of(String component, String name) {
        return new AgentComponentDescriptor(component, name, null, false, false);
    }

    public static AgentComponentDescriptor sandbox(String name, String isolationLevel) {
        if (!Set.of("none", "process", "os-sandbox", "container", "orchestrated")
                .contains(isolationLevel)) {
            throw new IllegalArgumentException("unknown sandbox isolation level");
        }
        return new AgentComponentDescriptor("sandbox_executor", name, isolationLevel, false, false);
    }

    public static AgentComponentDescriptor sandbox(
            String name, String isolationLevel, boolean supportsStreaming) {
        if (!Set.of("none", "process", "os-sandbox", "container", "orchestrated")
                .contains(isolationLevel)) {
            throw new IllegalArgumentException("unknown sandbox isolation level");
        }
        return new AgentComponentDescriptor(
                "sandbox_executor", name, isolationLevel, supportsStreaming, false);
    }

    public static AgentComponentDescriptor workflow(String name, boolean supportsStreaming) {
        return new AgentComponentDescriptor("workflow", name, null, supportsStreaming, false);
    }

    public static AgentComponentDescriptor mcpTransport(String name, boolean supportsNotifications) {
        return new AgentComponentDescriptor("mcp_transport", name, null, false, supportsNotifications);
    }

    @Override public String kind() { return "agent_component"; }

    @Override public ObjectNode toJson() {
        var descriptor = TypedExtensionSupport.baseDescriptor(kind())
                .put("component", component)
                .put("name", name);
        var capabilities = descriptor.putObject("capabilities");
        if (isolationLevel == null) capabilities.putNull("isolation_level");
        else capabilities.put("isolation_level", isolationLevel);
        capabilities.put("supports_streaming", supportsStreaming);
        capabilities.put("supports_notifications", supportsNotifications);
        return descriptor;
    }
}
