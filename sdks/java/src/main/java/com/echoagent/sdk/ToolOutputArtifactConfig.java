package com.echoagent.sdk;

/** Tool output artifact configuration without owning artifact writing. */
public record ToolOutputArtifactConfig(String rootDir, String retention, long thresholdBytes, Long maxAgeSecs) {
    public static ToolOutputArtifactConfig newConfig(String rootDir, String retention) {
        return new ToolOutputArtifactConfig(rootDir, retention, 1_048_576L, null);
    }

    public static ToolOutputArtifactConfig defaults() {
        String temp = System.getProperty("java.io.tmpdir");
        return new ToolOutputArtifactConfig(temp + "/echo_agent_artifacts/tool-logs", "temporary_1h", 1_048_576L, 3_600L);
    }

    public ToolOutputArtifactConfig thresholdBytes(long value) {
        return new ToolOutputArtifactConfig(rootDir, retention, Math.max(1L, value), maxAgeSecs);
    }

    public ToolOutputArtifactConfig maxAgeSecs(Long value) {
        return new ToolOutputArtifactConfig(rootDir, retention, thresholdBytes, value);
    }
}
