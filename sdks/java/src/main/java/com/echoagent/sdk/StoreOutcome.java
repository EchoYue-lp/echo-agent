package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.NullNode;

import java.math.BigInteger;
import java.util.Collection;
import java.util.List;

/** Operation-checked outcomes for Store callbacks. */
public interface StoreOutcome extends ExtensionOutcome {
    static StoreOutcome put() { return JsonExtensionOutcome.result("store_put", NullNode.instance); }
    static StoreOutcome get(StoreItem value) {
        return JsonExtensionOutcome.result("store_get", value == null ? NullNode.instance : value.toJson());
    }
    static StoreOutcome search(Collection<StoreItem> values) {
        var array = JsonSupport.MAPPER.createArrayNode();
        for (StoreItem value : values == null ? List.<StoreItem>of() : values) array.add(value.toJson());
        return JsonExtensionOutcome.result("store_search", array);
    }
    static StoreOutcome searchWith(Collection<StoreItem> values) {
        var array = JsonSupport.MAPPER.createArrayNode();
        for (StoreItem value : values == null ? List.<StoreItem>of() : values) array.add(value.toJson());
        return JsonExtensionOutcome.result("store_search_with", array);
    }
    static StoreOutcome deleted(boolean value) {
        return JsonExtensionOutcome.result("store_delete", JsonSupport.MAPPER.getNodeFactory().booleanNode(value));
    }
    static StoreOutcome namespaces(Collection<? extends Collection<String>> values) {
        var array = JsonSupport.MAPPER.createArrayNode();
        for (Collection<String> namespace : values == null ? List.<Collection<String>>of() : values) {
            array.add(TypedExtensionSupport.textArray(namespace, "namespace"));
        }
        return JsonExtensionOutcome.result("store_list_namespaces", array);
    }
    static StoreOutcome list(Collection<StoreItem> values) {
        var array = JsonSupport.MAPPER.createArrayNode();
        for (StoreItem value : values == null ? List.<StoreItem>of() : values) array.add(value.toJson());
        return JsonExtensionOutcome.result("store_list", array);
    }
    static StoreOutcome pruned(BigInteger count) {
        return JsonExtensionOutcome.result("store_prune_expired",
                JsonSupport.MAPPER.getNodeFactory().textNode(
                        TypedExtensionSupport.canonicalU64(count, "count")));
    }
    static StoreOutcome deduplicated(BigInteger count) {
        return JsonExtensionOutcome.result("store_dedup_by_content",
                JsonSupport.MAPPER.getNodeFactory().textNode(
                        TypedExtensionSupport.canonicalU64(count, "count")));
    }
    static StoreOutcome stream(WireHandle stream) { return JsonExtensionOutcome.stream(stream); }
    static StoreOutcome error(String code, String message, String retryable) {
        return JsonExtensionOutcome.error(code, message, retryable, null);
    }
}
