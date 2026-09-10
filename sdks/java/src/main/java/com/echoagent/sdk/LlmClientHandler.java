package com.echoagent.sdk;

import java.util.concurrent.CompletionStage;

/** Typed callback adapter for a Host-to-SDK LlmClient invocation. */
@FunctionalInterface
public interface LlmClientHandler {
    CompletionStage<? extends ExtensionOutcome> handle(LlmChatCall call, ExtensionCancellation cancellation);
}
