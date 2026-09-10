package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

/** Typed descriptor for a host-language {@code ContextCompressor}. */
public final class ContextCompressorDescriptor implements ExtensionDescriptor {
    private final String name;

    private ContextCompressorDescriptor(String name) {
        this.name = TypedExtensionSupport.requiredText(name, "name", 256);
    }

    public static ContextCompressorDescriptor named(String name) {
        return new ContextCompressorDescriptor(name);
    }

    @Override public String kind() { return "context_compressor"; }

    @Override public ObjectNode toJson() {
        return TypedExtensionSupport.baseDescriptor(kind()).put("name", name);
    }
}
