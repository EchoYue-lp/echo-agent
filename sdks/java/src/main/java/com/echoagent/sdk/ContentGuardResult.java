package com.echoagent.sdk;

import java.util.List;

/** Content-guard decisions projected without owning guard execution. */
public sealed interface ContentGuardResult
        permits ContentGuardResult.Pass, ContentGuardResult.Detected,
        ContentGuardResult.Rejected, ContentGuardResult.Redacted {
    boolean isRejected();

    static ContentGuardResult pass() { return new Pass(); }

    static ContentGuardResult detected(List<String> piiTypes) {
        return new Detected(List.copyOf(piiTypes));
    }

    static ContentGuardResult rejected(List<String> piiTypes) {
        return new Rejected(List.copyOf(piiTypes));
    }

    static ContentGuardResult redacted(String content) {
        return new Redacted(content);
    }

    record Pass() implements ContentGuardResult {
        @Override public boolean isRejected() { return false; }
    }

    record Detected(List<String> piiTypes) implements ContentGuardResult {
        public Detected { piiTypes = List.copyOf(piiTypes); }

        @Override public boolean isRejected() { return false; }
    }

    record Rejected(List<String> piiTypes) implements ContentGuardResult {
        public Rejected { piiTypes = List.copyOf(piiTypes); }

        @Override public boolean isRejected() { return true; }
    }

    record Redacted(String content) implements ContentGuardResult {
        public Redacted {
            if (content == null) throw new IllegalArgumentException("redacted content is required");
        }

        @Override public boolean isRejected() { return false; }
    }
}
