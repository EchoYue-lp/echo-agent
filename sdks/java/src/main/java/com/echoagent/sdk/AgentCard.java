package com.echoagent.sdk;

import java.util.List;

/** Immutable A2A agent card value. */
public final class AgentCard {
    private final String name;
    private final String description;
    private final String url;
    private final String version;
    private final AgentProvider provider;
    private final List<AgentSkill> skills;
    private final List<String> defaultInputModes;
    private final List<String> defaultOutputModes;
    private final AgentAuthentication authentication;
    private final AgentCapabilities capabilities;

    AgentCard(String name, String url, String description, String version,
              AgentProvider provider, List<AgentSkill> skills,
              List<String> defaultInputModes, List<String> defaultOutputModes,
              AgentAuthentication authentication, AgentCapabilities capabilities) {
        if (name == null || url == null) throw new IllegalArgumentException("agent name and url must not be null");
        this.name = name;
        this.description = description;
        this.url = url;
        this.version = version;
        this.provider = provider;
        this.skills = List.copyOf(skills == null ? List.of() : skills);
        this.defaultInputModes = List.copyOf(defaultInputModes == null ? List.of("text/plain") : defaultInputModes);
        this.defaultOutputModes = List.copyOf(defaultOutputModes == null ? List.of("text/plain") : defaultOutputModes);
        this.authentication = authentication;
        this.capabilities = capabilities == null ? new AgentCapabilities() : capabilities;
    }

    public static AgentCardBuilder builder(String name, String url) {
        return new AgentCardBuilder(name, url);
    }

    public String name() { return name; }
    public String description() { return description; }
    public String url() { return url; }
    public String version() { return version; }
    public AgentProvider provider() { return provider; }
    public List<AgentSkill> skills() { return skills; }
    public List<String> defaultInputModes() { return defaultInputModes; }
    public List<String> defaultOutputModes() { return defaultOutputModes; }
    public AgentAuthentication authentication() { return authentication; }
    public AgentCapabilities capabilities() { return capabilities; }

}
