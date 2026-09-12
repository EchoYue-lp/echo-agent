package com.echoagent.sdk;

import java.util.List;

/** Guard decisions projected without owning guard execution. */
public sealed interface GuardDecision
        permits GuardDecision.Pass, GuardDecision.Block, GuardDecision.Warn, GuardDecision.Transform {
    boolean isBlocked();

    static GuardDecision pass() { return new Pass(); }

    static GuardDecision block(String reason) { return new Block(reason); }

    static GuardDecision warn(List<String> reasons) { return new Warn(List.copyOf(reasons)); }

    static GuardDecision transform(String content, List<String> reasons) {
        return new Transform(content, List.copyOf(reasons));
    }

    record Pass() implements GuardDecision {
        @Override public boolean isBlocked() { return false; }
    }

    record Block(String reason) implements GuardDecision {
        public Block {
            if (reason == null) throw new IllegalArgumentException("block reason is required");
        }

        @Override public boolean isBlocked() { return true; }
    }

    record Warn(List<String> reasons) implements GuardDecision {
        public Warn { reasons = List.copyOf(reasons); }

        @Override public boolean isBlocked() { return false; }
    }

    record Transform(String content, List<String> reasons) implements GuardDecision {
        public Transform {
            if (content == null) throw new IllegalArgumentException("transformed content is required");
            reasons = List.copyOf(reasons);
        }

        @Override public boolean isBlocked() { return false; }
    }
}
