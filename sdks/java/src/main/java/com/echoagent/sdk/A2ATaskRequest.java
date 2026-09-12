package com.echoagent.sdk;

/** Immutable A2A JSON-RPC task request value. */
public final class A2ATaskRequest {
    private final String jsonrpc;
    private final String id;
    private final String method;
    private final A2ATaskParams params;

    private A2ATaskRequest(String id, String method, A2ATaskParams params) {
        if (id == null || method == null || params == null) throw new IllegalArgumentException("request fields must not be null");
        this.jsonrpc = "2.0";
        this.id = id;
        this.method = method;
        this.params = params;
    }

    public static A2ATaskRequest newRequest(String id, String method, A2ATaskParams params) {
        return new A2ATaskRequest(id, method, params);
    }
    public String jsonrpc() { return jsonrpc; }
    public String id() { return id; }
    public String method() { return method; }
    public A2ATaskParams params() { return params; }
}
