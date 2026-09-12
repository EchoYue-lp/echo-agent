package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

/** Provider capabilities advertised by a typed {@code LlmClient} descriptor. */
public final class LlmCapabilities {
    private final ObjectNode json;

    private LlmCapabilities(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }

    public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private boolean streamingToolCalls = true;
        private boolean namedSseEvents;
        private boolean reasoningContent = true;
        private boolean imageInput = true;
        private boolean systemAsTopLevel;
        private boolean ndjsonStreaming;
        private boolean toolSupport = true;
        private boolean structuredOutput = true;
        private boolean requiresVersionHeader;
        private boolean supportsParallelToolCalls = true;
        private boolean supportsToolChoiceNone = true;
        private String tokenizerName;

        public Builder streamingToolCalls(boolean value) { streamingToolCalls = value; return this; }
        public Builder namedSseEvents(boolean value) { namedSseEvents = value; return this; }
        public Builder reasoningContent(boolean value) { reasoningContent = value; return this; }
        public Builder imageInput(boolean value) { imageInput = value; return this; }
        public Builder systemAsTopLevel(boolean value) { systemAsTopLevel = value; return this; }
        public Builder ndjsonStreaming(boolean value) { ndjsonStreaming = value; return this; }
        public Builder toolSupport(boolean value) { toolSupport = value; return this; }
        public Builder structuredOutput(boolean value) { structuredOutput = value; return this; }
        public Builder requiresVersionHeader(boolean value) { requiresVersionHeader = value; return this; }
        public Builder supportsParallelToolCalls(boolean value) { supportsParallelToolCalls = value; return this; }
        public Builder supportsToolChoiceNone(boolean value) { supportsToolChoiceNone = value; return this; }
        public Builder tokenizerName(String value) { tokenizerName = value; return this; }

        public LlmCapabilities build() {
            var result = JsonSupport.MAPPER.createObjectNode()
                    .put("streaming_tool_calls", streamingToolCalls)
                    .put("named_sse_events", namedSseEvents)
                    .put("reasoning_content", reasoningContent)
                    .put("image_input", imageInput)
                    .put("system_as_top_level", systemAsTopLevel)
                    .put("ndjson_streaming", ndjsonStreaming)
                    .put("tool_support", toolSupport)
                    .put("structured_output", structuredOutput)
                    .put("requires_version_header", requiresVersionHeader)
                    .put("supports_parallel_tool_calls", supportsParallelToolCalls)
                    .put("supports_tool_choice_none", supportsToolChoiceNone);
            if (tokenizerName == null) result.putNull("tokenizer_name");
            else result.put("tokenizer_name", TypedExtensionSupport.requiredText(tokenizerName, "tokenizerName", 256));
            return new LlmCapabilities(result);
        }
    }
}
