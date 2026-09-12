package com.echoagent.sdk;

/** Provider-neutral thinking configuration and pure dialect projections. */
public final class ThinkingConfig {
    public enum Kind { DISABLED, LEVEL, BUDGET_TOKENS }
    private final Kind kind;
    private final ThinkingLevel level;
    private final Integer budgetTokens;

    private ThinkingConfig(Kind kind, ThinkingLevel level, Integer budgetTokens) {
        this.kind = kind; this.level = level; this.budgetTokens = budgetTokens;
    }
    public Kind kind() { return kind; }
    public ThinkingLevel level() { return level; }
    public Integer budgetTokens() { return budgetTokens; }
    public static ThinkingConfig disabled() { return new ThinkingConfig(Kind.DISABLED, null, null); }
    public static ThinkingConfig level(ThinkingLevel level) { return new ThinkingConfig(Kind.LEVEL, level, null); }
    public static ThinkingConfig budgetTokens(int value) {
        if (value < 0) throw new IllegalArgumentException("thinking budget must be non-negative");
        return new ThinkingConfig(Kind.BUDGET_TOKENS, null, value);
    }
    public static ThinkingConfig medium() { return level(ThinkingLevel.MEDIUM); }
    public static ThinkingConfig parseSpec(String spec) {
        String trimmed = spec.trim().toLowerCase(java.util.Locale.ROOT);
        if (trimmed.isEmpty() || trimmed.equals("auto") || trimmed.equals("default")) return null;
        if (trimmed.equals("disabled") || trimmed.equals("off") || trimmed.equals("false")) return disabled();
        if (trimmed.matches("[0-9]+")) return budgetTokens(Integer.parseInt(trimmed));
        ThinkingLevel level = ThinkingLevel.parse(trimmed);
        if (level != null) return level(level);
        throw new IllegalArgumentException("unrecognized thinking spec: '" + spec + "'");
    }
    public String toReasoningEffort() {
        if (kind == Kind.DISABLED) return "minimal";
        if (kind == Kind.BUDGET_TOKENS) return budgetEffort(budgetTokens, false);
        return level == ThinkingLevel.NONE ? "none" : level.toString();
    }
    public String toAnthropicEffort() {
        if (kind == Kind.DISABLED || level == ThinkingLevel.NONE || level == ThinkingLevel.MINIMAL) return null;
        return kind == Kind.BUDGET_TOKENS ? budgetEffort(budgetTokens, false) : level.toString();
    }
    public Integer toAnthropicBudget(int maxTokens) {
        if (maxTokens <= 1 || kind == Kind.DISABLED || level == ThinkingLevel.NONE || level == ThinkingLevel.MINIMAL) return null;
        int budget = kind == Kind.BUDGET_TOKENS ? budgetTokens : Math.round(maxTokens * switch (level) {
            case LOW -> .25f; case MEDIUM -> .5f; case HIGH -> .8f; case XHIGH -> .95f; case MAX -> .98f;
            case NONE, MINIMAL -> 0f;
        });
        return Math.min(budget, maxTokens - 1);
    }
    public boolean toEnableThinking() { return kind == Kind.BUDGET_TOKENS || (level != ThinkingLevel.NONE && level != ThinkingLevel.MINIMAL); }
    public String toGlmThinkingType() { return toEnableThinking() ? "enabled" : "disabled"; }
    public String toGlmReasoningEffort() {
        if (kind == Kind.DISABLED) return "none";
        return kind == Kind.BUDGET_TOKENS ? budgetEffort(budgetTokens, true) : level.toString();
    }
    private static String budgetEffort(int tokens, boolean glm) {
        if (tokens < 4_000) return "low";
        if (tokens < 12_000) return "medium";
        if (tokens < 24_000) return "high";
        if (glm || tokens >= 48_000) return "max";
        return "xhigh";
    }
}
