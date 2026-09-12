package com.echoagent.sdk;

import java.util.List;

/** Permission rule behavior values projected without owning evaluation. */
public sealed interface RuleBehavior permits RuleBehavior.Allow, RuleBehavior.Deny, RuleBehavior.Ask {
    RuleDecision toDecision();

    static RuleBehavior allow() { return new Allow(); }

    static RuleBehavior deny(String reason) { return new Deny(reason); }

    static RuleBehavior ask(List<String> suggestions) { return new Ask(List.copyOf(suggestions)); }

    static RuleBehavior parse(String value) {
        if (value == null) throw new IllegalArgumentException("rule behavior must be text");
        return switch (value) {
            case "allow" -> allow();
            case "deny" -> deny("denied by rule");
            case "ask" -> ask(List.of("allow", "deny"));
            default -> throw new IllegalArgumentException("unknown permission rule behavior: " + value);
        };
    }

    record Allow() implements RuleBehavior {
        @Override public RuleDecision toDecision() { return RuleDecision.allow(); }
    }

    record Deny(String reason) implements RuleBehavior {
        public Deny { if (reason == null) throw new IllegalArgumentException("deny reason is required"); }

        @Override public RuleDecision toDecision() { return RuleDecision.deny(reason); }
    }

    record Ask(List<String> suggestions) implements RuleBehavior {
        public Ask { suggestions = List.copyOf(suggestions); }

        @Override public RuleDecision toDecision() { return RuleDecision.ask(suggestions); }
    }

    sealed interface RuleDecision permits RuleDecision.Allow, RuleDecision.Deny, RuleDecision.Ask {
        static RuleDecision allow() { return new Allow(); }

        static RuleDecision deny(String reason) { return new Deny(reason); }

        static RuleDecision ask(List<String> suggestions) { return new Ask(List.copyOf(suggestions)); }

        record Allow() implements RuleDecision {}
        record Deny(String reason) implements RuleDecision {}
        record Ask(List<String> suggestions) implements RuleDecision {
            public Ask { suggestions = List.copyOf(suggestions); }
        }
    }
}
