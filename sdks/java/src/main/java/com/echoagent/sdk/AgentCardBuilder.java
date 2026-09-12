package com.echoagent.sdk;

import java.util.ArrayList;
import java.util.List;

/** Fluent local builder for an A2A agent card. */
public final class AgentCardBuilder {
    private final String name;
    private final String url;
    private String description;
    private String version;
    private AgentProvider provider;
    private final List<AgentSkill> skills = new ArrayList<>();
    private List<String> inputModes = List.of("text/plain");
    private List<String> outputModes = List.of("text/plain");
    private AgentAuthentication authentication;
    private boolean streaming;
    private boolean pushNotifications;

    AgentCardBuilder(String name, String url) {
        if (name == null || url == null) throw new IllegalArgumentException("agent name and url must not be null");
        this.name = name;
        this.url = url;
    }

    public AgentCardBuilder description(String value) {
        if (value == null) throw new IllegalArgumentException("agent description must not be null");
        description = value;
        return this;
    }
    public AgentCardBuilder version(String value) {
        if (value == null) throw new IllegalArgumentException("agent version must not be null");
        version = value;
        return this;
    }
    public AgentCardBuilder provider(AgentProvider value) {
        if (value == null) throw new IllegalArgumentException("provider must not be null");
        provider = value;
        return this;
    }
    public AgentCardBuilder skill(AgentSkill value) {
        if (value == null) throw new IllegalArgumentException("skill must not be null");
        skills.add(value);
        return this;
    }
    public AgentCardBuilder skills(List<AgentSkill> values) {
        if (values == null || values.stream().anyMatch(value -> value == null)) {
            throw new IllegalArgumentException("skills must not be null");
        }
        skills.addAll(values);
        return this;
    }
    public AgentCardBuilder inputModes(List<String> values) {
        inputModes = textList(values, "input modes");
        return this;
    }
    public AgentCardBuilder outputModes(List<String> values) {
        outputModes = textList(values, "output modes");
        return this;
    }
    public AgentCardBuilder authentication(AgentAuthentication value) {
        if (value == null) throw new IllegalArgumentException("authentication must not be null");
        authentication = value;
        return this;
    }
    public AgentCardBuilder streaming() { streaming = true; return this; }
    public AgentCardBuilder pushNotifications() { pushNotifications = true; return this; }
    public AgentCard build() {
        return new AgentCard(name, url, description, version, provider, skills, inputModes,
                outputModes, authentication, new AgentCapabilities(streaming, pushNotifications, false));
    }

    private static List<String> textList(List<String> values, String field) {
        if (values == null || values.stream().anyMatch(value -> value == null)) {
            throw new IllegalArgumentException(field + " must be a non-null string list");
        }
        return List.copyOf(values);
    }
}
