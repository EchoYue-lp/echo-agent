package com.echoagent.sdk;

/** LLM timeout policy in milliseconds; zero disables a boundary. */
public final class LlmTimeouts {
    private final Long request, firstChunk, idle, overall;
    private LlmTimeouts(Long request, Long firstChunk, Long idle, Long overall) {
        this.request = request; this.firstChunk = firstChunk; this.idle = idle; this.overall = overall;
    }
    public static LlmTimeouts defaults() { return new LlmTimeouts(60_000L, 30_000L, 30_000L, null); }
    public Long requestTimeout() { return request; }
    public Long firstChunkTimeout() { return firstChunk; }
    public Long idleTimeout() { return idle; }
    public Long overallTimeout() { return overall; }
    public LlmTimeouts withRequestTimeout(long ms) { return new LlmTimeouts(ms > 0 ? ms : null, firstChunk, idle, overall); }
    public LlmTimeouts withoutRequestTimeout() { return new LlmTimeouts(null, firstChunk, idle, overall); }
    public LlmTimeouts withFirstChunkTimeout(long ms) { return new LlmTimeouts(request, ms > 0 ? ms : null, idle, overall); }
    public LlmTimeouts withoutFirstChunkTimeout() { return new LlmTimeouts(request, null, idle, overall); }
    public LlmTimeouts withIdleTimeout(long ms) { return new LlmTimeouts(request, firstChunk, ms > 0 ? ms : null, overall); }
    public LlmTimeouts withoutIdleTimeout() { return new LlmTimeouts(request, firstChunk, null, overall); }
    public LlmTimeouts withOverallTimeout(long ms) { return new LlmTimeouts(request, firstChunk, idle, ms > 0 ? ms : null); }
    public LlmTimeouts withoutOverallTimeout() { return new LlmTimeouts(request, firstChunk, idle, null); }
}
