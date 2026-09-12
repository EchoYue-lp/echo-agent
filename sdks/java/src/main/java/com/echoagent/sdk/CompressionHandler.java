package com.echoagent.sdk;

import java.util.concurrent.CompletionStage;

/** Typed callback adapter for a Host-to-SDK ContextCompressor invocation. */
@FunctionalInterface
public interface CompressionHandler {
    CompletionStage<? extends CompressionOutcome> handle(
            CompressionCall call, ExtensionCancellation cancellation);
}
