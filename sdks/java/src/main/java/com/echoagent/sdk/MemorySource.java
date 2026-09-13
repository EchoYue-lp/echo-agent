package com.echoagent.sdk;

/** Memory source confidence and recall policies projected from Rust. */
public enum MemorySource {
    USER_CORRECTION("user_correction", 0.90, 0.90),
    ERROR_RESOLUTION("error_resolution", 0.85, 0.70),
    REPEATED_WORKFLOW("repeated_workflow", 0.75, 0.60),
    EXPLICIT_SAVE("explicit_save", 1.00, 0.80),
    AUTO_EXTRACTED("auto_extracted", 0.60, 0.40),
    L3_PROMOTION("l3_promotion", 0.50, 0.40);

    private final String wireName;
    private final double defaultConfidence;
    private final double defaultRecallWeight;

    MemorySource(String wireName, double defaultConfidence, double defaultRecallWeight) {
        this.wireName = wireName;
        this.defaultConfidence = defaultConfidence;
        this.defaultRecallWeight = defaultRecallWeight;
    }

    public double defaultConfidence() { return defaultConfidence; }
    public double defaultRecallWeight() { return defaultRecallWeight; }
    public String wireName() { return wireName; }

    @Override public String toString() { return wireName; }
}
