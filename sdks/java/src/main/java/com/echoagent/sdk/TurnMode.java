package com.echoagent.sdk;

/** Agent turn stream modes projected as local values. */
public enum TurnMode {
    CHAT("chat"), EXECUTE("execute");

    private final String wireName;
    TurnMode(String wireName) { this.wireName = wireName; }
    public String asStr() { return wireName; }
    @Override public String toString() { return wireName; }
}
