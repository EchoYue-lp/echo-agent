package com.echoagent.sdk;

import java.util.concurrent.CompletionStage;

/** Typed callback adapter for a Host-to-SDK Critic invocation. */
@FunctionalInterface
public interface CriticHandler {
    CompletionStage<? extends ExtensionOutcome> handle(CriticCall call, ExtensionCancellation cancellation);
}
