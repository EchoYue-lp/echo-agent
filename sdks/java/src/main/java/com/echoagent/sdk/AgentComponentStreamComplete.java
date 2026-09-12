package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;

/** Closed terminal event set for Agent component streams. */
public sealed interface AgentComponentStreamComplete permits
        AgentComponentStreamComplete.SandboxComplete,
        AgentComponentStreamComplete.SandboxFailed,
        AgentComponentStreamComplete.WorkflowCompleted {

    ObjectNode toJson();

    record SandboxComplete(JsonNode result) implements AgentComponentStreamComplete {
        public SandboxComplete {
            if (result == null) throw new IllegalArgumentException("result must not be null");
        }
        @Override public ObjectNode toJson() {
            var terminal = JsonSupport.MAPPER.createObjectNode().put("terminal", "complete");
            terminal.set("result", result.deepCopy());
            return envelope("sandbox", terminal);
        }
    }

    record SandboxFailed(String kind, String message) implements AgentComponentStreamComplete {
        public SandboxFailed {
            if (!java.util.Set.of("cancelled", "io_error").contains(kind)) {
                throw new IllegalArgumentException("sandbox failure kind is invalid");
            }
            if (message == null) throw new IllegalArgumentException("message must not be null");
        }
        @Override public ObjectNode toJson() {
            var terminal = JsonSupport.MAPPER.createObjectNode().put("terminal", "failed");
            terminal.set("failure", JsonSupport.MAPPER.createObjectNode()
                    .put("kind", kind).put("message", message));
            return envelope("sandbox", terminal);
        }
    }

    record WorkflowCompleted(String result, BigInteger totalSteps, BigInteger seconds, int nanos)
            implements AgentComponentStreamComplete {
        @Override public ObjectNode toJson() {
            if (result == null) throw new IllegalArgumentException("result must not be null");
            if (nanos < 0 || nanos >= 1_000_000_000) {
                throw new IllegalArgumentException("duration nanos must be below one second");
            }
            var terminal = JsonSupport.MAPPER.createObjectNode().put("result", result)
                    .put("total_steps", u64(totalSteps, "totalSteps"));
            terminal.set("elapsed", JsonSupport.MAPPER.createObjectNode()
                    .put("seconds", u64(seconds, "seconds")).put("nanos", nanos));
            return envelope("workflow", terminal);
        }
    }

    private static ObjectNode envelope(String component, ObjectNode terminal) {
        var value = JsonSupport.MAPPER.createObjectNode().put("component", component);
        value.set("terminal", terminal);
        return value;
    }

    private static String u64(BigInteger value, String field) {
        if (value == null) throw new IllegalArgumentException(field + " must not be null");
        return TypedExtensionSupport.canonicalU64(value.toString(), field);
    }
}
