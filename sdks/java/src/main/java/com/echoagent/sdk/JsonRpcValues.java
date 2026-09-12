package com.echoagent.sdk;

/** MCP JSON-RPC request/notification values without owning transport. */
public final class JsonRpcValues {
    private JsonRpcValues() {}

    public record Request(String jsonrpc, Object id, String method, Object params) {
        public static Request newRequest(String method, Object params) {
            return new Request("2.0", null, method, params);
        }
    }

    public record Notification(String jsonrpc, String method, Object params) {
        public static Notification newNotification(String method, Object params) {
            return new Notification("2.0", method, params);
        }
    }
}
