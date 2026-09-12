package com.echoagent.sdk;

import java.util.concurrent.atomic.AtomicBoolean;

/** Cancellation signal shared with one Host-to-SDK extension invocation. */
public final class ExtensionCancellation {
    private final AtomicBoolean cancelled = new AtomicBoolean();

    public boolean isCancelled() { return cancelled.get(); }
    public void cancel() { cancelled.set(true); }
}
