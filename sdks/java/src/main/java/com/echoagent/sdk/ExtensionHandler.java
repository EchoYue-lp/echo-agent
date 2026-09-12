package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.concurrent.CompletionStage;

/** Handles one Host-to-SDK extension invocation. Return a serialized outcome object. */
@FunctionalInterface
public interface ExtensionHandler {
    CompletionStage<JsonNode> handle(JsonNode call, ExtensionCancellation cancellation);
}
