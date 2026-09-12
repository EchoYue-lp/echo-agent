package com.echoagent.sdk;

/** Token budget configuration projected without constructing an Agent. */
public final class TokenBudgetConfig {
    private final Long totalWindow;
    private final double systemPct;
    private final double toolPct;
    private final double outputPct;
    private final double safetyPct;
    private final boolean enabled;

    private TokenBudgetConfig(Long totalWindow, double systemPct, double toolPct, double outputPct, double safetyPct, boolean enabled) {
        this.totalWindow = totalWindow; this.systemPct = systemPct; this.toolPct = toolPct;
        this.outputPct = outputPct; this.safetyPct = safetyPct; this.enabled = enabled;
    }
    public static TokenBudgetConfig enabled() { return new TokenBudgetConfig(null, .1, .05, .1, .1, true); }
    public static TokenBudgetConfig disabled() { return new TokenBudgetConfig(null, .1, .05, .1, .1, false); }
    public TokenBudgetConfig withTotalWindow(long window) {
        if (window <= 0) throw new IllegalArgumentException("token budget total window must be greater than zero");
        return new TokenBudgetConfig(window, systemPct, toolPct, outputPct, safetyPct, enabled);
    }
    public TokenBudget build(long fallbackWindow) {
        return TokenBudget.newBudget(totalWindow == null ? fallbackWindow : totalWindow)
                .withAllocations(systemPct, toolPct, outputPct, safetyPct);
    }
    public boolean isEnabled() { return enabled; }
}
