package com.echoagent.sdk;

/** Memory classifications and promotion policies projected from Rust. */
public enum MemoryType {
    USER_PREFERENCE("user_preference", 0.85, false, true),
    PROJECT_FACT("project_fact", 0.60, false, true),
    ARCHITECTURE_DECISION("architecture_decision", 0.80, false, true),
    DEBUGGING_LESSON("debugging_lesson", 0.55, true, false),
    ERROR_RESOLUTION("error_resolution", 0.50, false, false),
    COMMAND_PATTERN("command_pattern", 0.40, false, false),
    TOOL_USAGE("tool_usage", 0.50, false, false),
    WORKFLOW_PATTERN("workflow_pattern", 0.60, true, false),
    SKILL_CANDIDATE("skill_candidate", 0.55, true, false),
    DEPRECATED_NOTE("deprecated_note", 0.10, false, false);

    private final String wireName;
    private final double defaultStability;
    private final boolean skillEligible;
    private final boolean ruleEligible;

    MemoryType(String wireName, double defaultStability, boolean skillEligible, boolean ruleEligible) {
        this.wireName = wireName;
        this.defaultStability = defaultStability;
        this.skillEligible = skillEligible;
        this.ruleEligible = ruleEligible;
    }

    public double defaultStability() { return defaultStability; }
    public boolean isSkillEligible() { return skillEligible; }
    public boolean isRuleEligible() { return ruleEligible; }
    public String wireName() { return wireName; }

    @Override public String toString() { return wireName; }
}
