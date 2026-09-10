package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Collection;
import java.util.List;

/** Typed descriptor for a host-language {@code Store} implementation. */
public final class StoreDescriptor implements ExtensionDescriptor {
    private final ObjectNode json;

    private StoreDescriptor(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }

    @Override public String kind() { return "store"; }
    @Override public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private List<String> searchModes = List.of();

        public Builder searchModes(Collection<String> values) {
            searchModes = List.copyOf(values == null ? List.of() : values);
            for (String value : searchModes) {
                if (!List.of("keyword", "semantic", "hybrid").contains(value)) {
                    throw new IllegalArgumentException("unknown store search mode: " + value);
                }
            }
            return this;
        }

        public StoreDescriptor build() {
            var result = TypedExtensionSupport.baseDescriptor("store");
            result.set("search_modes", TypedExtensionSupport.textArray(searchModes, "search mode"));
            return new StoreDescriptor(result);
        }
    }
}
