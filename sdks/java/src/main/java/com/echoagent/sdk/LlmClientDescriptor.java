package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

/** Typed descriptor for a host-language {@code LlmClient} implementation. */
public final class LlmClientDescriptor implements ExtensionDescriptor {
    private final ObjectNode json;

    private LlmClientDescriptor(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }

    @Override public String kind() { return "llm_client"; }
    @Override public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private String modelName;
        private boolean supportsStreaming;
        private LlmCapabilities capabilities = LlmCapabilities.builder().build();

        public Builder modelName(String value) { modelName = value; return this; }
        public Builder supportsStreaming(boolean value) { supportsStreaming = value; return this; }
        public Builder capabilities(LlmCapabilities value) {
            capabilities = java.util.Objects.requireNonNull(value, "capabilities");
            return this;
        }

        public LlmClientDescriptor build() {
            var result = TypedExtensionSupport.baseDescriptor("llm_client");
            result.put("model_name", TypedExtensionSupport.requiredText(modelName, "modelName", 256));
            result.put("supports_streaming", supportsStreaming);
            result.set("capabilities", capabilities.toJson());
            return new LlmClientDescriptor(result);
        }
    }
}
