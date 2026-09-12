package com.echoagent.sdk;

/** A2A provider value. */
public final class AgentProvider {
    private final String organization;
    private final String url;

    private AgentProvider(String organization, String url) {
        if (organization == null) throw new IllegalArgumentException("organization must not be null");
        this.organization = organization;
        this.url = url;
    }

    public static AgentProvider newProvider(String organization) { return new AgentProvider(organization, null); }
    public AgentProvider withUrl(String value) {
        if (value == null) throw new IllegalArgumentException("provider url must not be null");
        return new AgentProvider(organization, value);
    }
    public String organization() { return organization; }
    public String url() { return url; }
}
