package com.echoagent.sdk;

import java.util.List;

/** Team collaboration strategies projected without owning Team execution. */
public sealed interface TeamStrategy
        permits TeamStrategy.ManagerSubagent, TeamStrategy.Pipeline, TeamStrategy.Debate, TeamStrategy.Swarm {
    String name();
    String description();

    static TeamStrategy manager() { return new ManagerSubagent(); }
    static TeamStrategy pipeline(List<String> members) { return new Pipeline(List.copyOf(members)); }
    static TeamStrategy debate(String judge, List<String> debaters) { return new Debate(judge, List.copyOf(debaters)); }
    static TeamStrategy swarm(String reducer) { return new Swarm(reducer); }

    record ManagerSubagent() implements TeamStrategy {
        public String name() { return "manager_subagent"; }
        public String description() { return "Manager plans typed tasks, Subagents execute them, and the manager synthesizes"; }
    }
    record Pipeline(List<String> members) implements TeamStrategy {
        public Pipeline { members = List.copyOf(members); }
        public String name() { return "pipeline"; }
        public String description() { return "Subagents execute in sequence"; }
    }
    record Debate(String judge, List<String> debaters) implements TeamStrategy {
        public Debate { debaters = List.copyOf(debaters); }
        public String name() { return "debate"; }
        public String description() { return "Debaters propose independently and a judge synthesizes"; }
    }
    record Swarm(String reducer) implements TeamStrategy {
        public String name() { return "swarm"; }
        public String description() { return "Subagents inspect independently and a reducer synthesizes"; }
    }
}
