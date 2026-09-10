package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;
import java.util.Collection;
import java.util.List;

/** Typed descriptor for a host-language {@code Tool} implementation. */
public final class ToolDescriptor implements ExtensionDescriptor {
    private final ObjectNode json;

    private ToolDescriptor(ObjectNode json) {
        this.json = json;
    }

    public static Builder builder() { return new Builder(); }

    @Override public String kind() { return "tool"; }

    @Override public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private String name;
        private String description;
        private JsonNode parameters;
        private String schemaRevision = "1";
        private boolean supportsStreaming;
        private boolean exemptFromBatchTimeout;
        private boolean allowsParallelBatchExecution = true;
        private boolean managesOwnTimeout;
        private String riskLevel = "standard";
        private List<String> requiredPermissions = List.of();
        private List<String> requiredInputModalities = List.of();

        public Builder name(String value) { name = value; return this; }
        public Builder description(String value) { description = value; return this; }

        /** Supplies an already encoded WireValue parameter schema. */
        public Builder parameters(JsonNode value) {
            parameters = TypedExtensionSupport.copyObject(value, "parameters");
            return this;
        }

        /** Encodes a Java value using the SDK's lossless WireValue algebra. */
        public Builder parametersValue(Object value) {
            parameters = TypedExtensionSupport.wireValue(value, "parameters");
            return this;
        }

        public Builder schemaRevision(BigInteger value) {
            schemaRevision = TypedExtensionSupport.canonicalU64(value, "schemaRevision");
            return this;
        }

        public Builder schemaRevision(String value) {
            schemaRevision = TypedExtensionSupport.canonicalU64(value, "schemaRevision");
            return this;
        }

        public Builder supportsStreaming(boolean value) { supportsStreaming = value; return this; }
        public Builder exemptFromBatchTimeout(boolean value) { exemptFromBatchTimeout = value; return this; }
        public Builder allowsParallelBatchExecution(boolean value) {
            allowsParallelBatchExecution = value;
            return this;
        }
        public Builder managesOwnTimeout(boolean value) { managesOwnTimeout = value; return this; }

        public Builder riskLevel(String value) {
            if (!List.of("read_only", "standard", "dangerous").contains(value)) {
                throw new IllegalArgumentException("riskLevel must be read_only, standard or dangerous");
            }
            riskLevel = value;
            return this;
        }

        public Builder requiredPermissions(Collection<String> values) {
            requiredPermissions = List.copyOf(values == null ? List.of() : values);
            for (String value : requiredPermissions) {
                if (!List.of("read", "write", "network", "execute", "sensitive").contains(value)) {
                    throw new IllegalArgumentException("unknown tool permission: " + value);
                }
            }
            return this;
        }

        public Builder requiredInputModalities(Collection<String> values) {
            requiredInputModalities = List.copyOf(values == null ? List.of() : values);
            for (String value : requiredInputModalities) {
                if (!List.of("text", "image", "audio", "video").contains(value)) {
                    throw new IllegalArgumentException("unknown input modality: " + value);
                }
            }
            return this;
        }

        public ToolDescriptor build() {
            var result = TypedExtensionSupport.baseDescriptor("tool");
            result.put("name", TypedExtensionSupport.requiredText(name, "name", 128));
            result.put("description", TypedExtensionSupport.requiredText(description, "description", 8192));
            result.set("parameters", parameters == null
                    ? JsonSupport.wire(java.util.Map.of()) : parameters.deepCopy());
            result.put("schema_revision", schemaRevision);
            result.put("supports_streaming", supportsStreaming);
            result.put("exempt_from_batch_timeout", exemptFromBatchTimeout);
            result.put("allows_parallel_batch_execution", allowsParallelBatchExecution);
            result.put("manages_own_timeout", managesOwnTimeout);
            result.put("risk_level", riskLevel);
            result.set("required_permissions", TypedExtensionSupport.textArray(requiredPermissions, "permission"));
            result.set("required_input_modalities", TypedExtensionSupport.textArray(requiredInputModalities, "modality"));
            return new ToolDescriptor(result);
        }
    }
}
