package com.echoagent.sdk;

import java.util.concurrent.CompletionStage;

/** Typed callback for a Host-consumed Agent infrastructure component. */
@FunctionalInterface
public interface AgentComponentHandler {
    CompletionStage<? extends ExtensionOutcome> handle(
            AgentComponentCall call, ExtensionCancellation cancellation);
}
