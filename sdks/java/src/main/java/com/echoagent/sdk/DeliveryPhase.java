package com.echoagent.sdk;

/** Durable delivery phases projected without owning the delivery ledger. */
public enum DeliveryPhase {
    PERSISTED("persisted"), CLAIMED("claimed"), EFFECT_STARTED("effect_started"),
    MAILBOX_ACCEPTED("mailbox_accepted"), DRAINED("drained"),
    DEFERRED("deferred"), TURN_SETTLED("turn_settled");

    private final String wireName;

    DeliveryPhase(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override
    public String toString() { return wireName; }
}
