package com.echoagent.sdk;

import java.util.concurrent.CompletionStage;

/** Typed callback adapter for a Host-to-SDK Store invocation. */
@FunctionalInterface
public interface StoreHandler {
    CompletionStage<? extends ExtensionOutcome> handle(StoreCall call, ExtensionCancellation cancellation);
}
