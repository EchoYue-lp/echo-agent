package com.echoagent.sdk;

import java.util.concurrent.CompletionStage;

/** Typed callback adapter for a Host-to-SDK Tool invocation. */
@FunctionalInterface
public interface ToolHandler {
    CompletionStage<? extends ExtensionOutcome> handle(ToolCall call, ExtensionCancellation cancellation);
}
