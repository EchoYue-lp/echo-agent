package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.Objects;

/** Immutable intervention decision values; callback execution remains Host-owned. */
public final class InterventionResult {
    private final boolean block;
    private final String blockReason;
    private final String injectedContext;
    private final String redirectTo;
    private final boolean cancel;
    private final JsonNode modifiedArgs;

    private InterventionResult(boolean block, String blockReason, String injectedContext,
            String redirectTo, boolean cancel, JsonNode modifiedArgs) {
        this.block = block;
        this.blockReason = blockReason;
        this.injectedContext = injectedContext;
        this.redirectTo = redirectTo;
        this.cancel = cancel;
        this.modifiedArgs = modifiedArgs;
    }

    public static InterventionResult allow() { return new InterventionResult(false, null, null, null, false, null); }

    public static InterventionResult block(String reason) {
        if (reason == null || reason.isBlank()) throw new IllegalArgumentException("block reason must not be empty");
        return new InterventionResult(true, reason, null, null, false, null);
    }

    public static InterventionResult inject(String context) {
        return new InterventionResult(false, null, context, null, false, null);
    }

    public static InterventionResult cancel() { return new InterventionResult(false, null, null, null, true, null); }

    public static InterventionResult modifyArgs(JsonNode args) {
        return new InterventionResult(false, null, null, null, false, Objects.requireNonNull(args, "args"));
    }

    public boolean block() { return block; }
    public String blockReason() { return blockReason; }
    public String injectedContext() { return injectedContext; }
    public String redirectTo() { return redirectTo; }
    public boolean isCancelled() { return cancel; }
    public JsonNode modifiedArgs() { return modifiedArgs; }
}
