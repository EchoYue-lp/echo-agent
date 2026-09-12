package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

/** Typed descriptor for a host-language {@code Critic} implementation. */
public final class CriticDescriptor implements ExtensionDescriptor {
    private final ObjectNode json;

    private CriticDescriptor(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }

    @Override public String kind() { return "critic"; }
    @Override public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private String name;

        public Builder name(String value) { name = value; return this; }

        public CriticDescriptor build() {
            var result = TypedExtensionSupport.baseDescriptor("critic");
            result.put("name", TypedExtensionSupport.requiredText(name, "name", 256));
            return new CriticDescriptor(result);
        }
    }
}
