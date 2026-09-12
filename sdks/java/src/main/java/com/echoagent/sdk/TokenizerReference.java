package com.echoagent.sdk;

/** Temporary owner-checked Host tokenizer supplied to one compression callback. */
public record TokenizerReference(WireHandle resource, String ownerSessionId) {
    public TokenizerReference {
        if (resource == null || !"facade_resource".equals(resource.kind())) {
            throw new IllegalArgumentException("tokenizer resource must be a facade resource");
        }
        if (ownerSessionId == null || ownerSessionId.isBlank()) {
            throw new IllegalArgumentException("tokenizer ownerSessionId must be non-empty");
        }
    }
}
