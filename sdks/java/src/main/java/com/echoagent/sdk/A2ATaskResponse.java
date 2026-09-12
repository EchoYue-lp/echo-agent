package com.echoagent.sdk;

/** Immutable A2A JSON-RPC task response value. */
public final class A2ATaskResponse {
    private final String jsonrpc;
    private final String id;
    private final A2ATask result;
    private final A2AError error;

    private A2ATaskResponse(String id, A2ATask result, A2AError error) {
        this.jsonrpc = "2.0";
        this.id = id;
        this.result = result;
        this.error = error;
    }

    public static A2ATaskResponse newResponse(String id, A2ATask result, A2AError error) {
        return new A2ATaskResponse(id, result, error);
    }
    public String jsonrpc() { return jsonrpc; }
    public String id() { return id; }
    public A2ATask result() { return result; }
    public A2AError error() { return error; }
}
