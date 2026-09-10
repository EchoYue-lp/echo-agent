package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;

/** Closed non-terminal event set for Agent component streams. */
public sealed interface AgentComponentStreamChunk permits
        AgentComponentStreamChunk.SandboxOutput,
        AgentComponentStreamChunk.WorkflowNodeStart,
        AgentComponentStreamChunk.WorkflowNodeEnd,
        AgentComponentStreamChunk.WorkflowToken,
        AgentComponentStreamChunk.WorkflowNodeError {

    ObjectNode toJson();

    record SandboxOutput(String channel, String chunk) implements AgentComponentStreamChunk {
        public SandboxOutput {
            if (!java.util.Set.of("stdout", "stderr").contains(channel)) {
                throw new IllegalArgumentException("sandbox channel must be stdout or stderr");
            }
            if (chunk == null) throw new IllegalArgumentException("chunk must not be null");
        }
        @Override public ObjectNode toJson() {
            var event = JsonSupport.MAPPER.createObjectNode()
                    .put("event", "output").put("channel", channel).put("chunk", chunk);
            return envelope("sandbox", event);
        }
    }

    record WorkflowNodeStart(String nodeName, BigInteger stepIndex)
            implements AgentComponentStreamChunk {
        @Override public ObjectNode toJson() {
            var event = JsonSupport.MAPPER.createObjectNode().put("event", "node_start")
                    .put("node_name", nodeName)
                    .put("step_index", u64(stepIndex, "stepIndex"));
            return envelope("workflow", event);
        }
    }

    record WorkflowNodeEnd(String nodeName, BigInteger stepIndex, BigInteger seconds, int nanos)
            implements AgentComponentStreamChunk {
        @Override public ObjectNode toJson() {
            var event = JsonSupport.MAPPER.createObjectNode().put("event", "node_end")
                    .put("node_name", nodeName)
                    .put("step_index", u64(stepIndex, "stepIndex"));
            event.set("elapsed", duration(seconds, nanos));
            return envelope("workflow", event);
        }
    }

    record WorkflowToken(String nodeName, String token) implements AgentComponentStreamChunk {
        @Override public ObjectNode toJson() {
            return envelope("workflow", JsonSupport.MAPPER.createObjectNode()
                    .put("event", "token").put("node_name", nodeName).put("token", token));
        }
    }

    record WorkflowNodeError(String nodeName, String error) implements AgentComponentStreamChunk {
        @Override public ObjectNode toJson() {
            return envelope("workflow", JsonSupport.MAPPER.createObjectNode()
                    .put("event", "node_error").put("node_name", nodeName).put("error", error));
        }
    }

    private static ObjectNode envelope(String component, ObjectNode event) {
        var value = JsonSupport.MAPPER.createObjectNode().put("component", component);
        value.set("event", event);
        return value;
    }

    private static String u64(BigInteger value, String field) {
        if (value == null) throw new IllegalArgumentException(field + " must not be null");
        return TypedExtensionSupport.canonicalU64(value.toString(), field);
    }

    private static ObjectNode duration(BigInteger seconds, int nanos) {
        if (nanos < 0 || nanos >= 1_000_000_000) {
            throw new IllegalArgumentException("duration nanos must be below one second");
        }
        return JsonSupport.MAPPER.createObjectNode()
                .put("seconds", u64(seconds, "seconds")).put("nanos", nanos);
    }
}
