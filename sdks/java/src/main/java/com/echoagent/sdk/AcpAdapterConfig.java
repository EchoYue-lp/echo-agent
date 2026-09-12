package com.echoagent.sdk;

import java.time.Duration;

/** ACP adapter configuration projected without owning the adapter runtime. */
public record AcpAdapterConfig(
        String name,
        String title,
        String version,
        long maxSessions,
        long maxPromptChars,
        long maxUpdateChars,
        long maxUpdatesPerTurn,
        long maxTotalUpdateChars,
        long maxExtensionConcurrency,
        Duration shutdownTimeout) {

    public static AcpAdapterConfig defaults() {
        return new AcpAdapterConfig(
                "echo-agent", "echo-agent", "0.2.0", 128L, 1_000_000L, 1_000_000L,
                10_000L, 8_000_000L, 8L, Duration.ofSeconds(5));
    }

    public void validate() {
        if (name == null || name.isBlank() || title == null || title.isBlank()
                || version == null || version.isBlank()) {
            throw new IllegalArgumentException("ACP adapter name, title, and version must not be empty");
        }
        if (maxSessions <= 0 || maxPromptChars <= 0 || maxUpdateChars <= 0
                || maxUpdatesPerTurn <= 0 || maxTotalUpdateChars <= 0
                || maxExtensionConcurrency <= 0) {
            throw new IllegalArgumentException("ACP adapter resource limits must be positive");
        }
        if (shutdownTimeout == null || shutdownTimeout.isZero() || shutdownTimeout.isNegative()) {
            throw new IllegalArgumentException("ACP adapter shutdown timeout must be positive");
        }
    }
}
