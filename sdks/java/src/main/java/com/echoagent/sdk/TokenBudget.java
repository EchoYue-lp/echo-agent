package com.echoagent.sdk;

/** Token budget policy projected as an immutable local value. */
public final class TokenBudget {
    private final long totalWindow;
    private final double systemPct;
    private final double toolPct;
    private final double outputPct;
    private final double safetyPct;

    private TokenBudget(long totalWindow, double systemPct, double toolPct, double outputPct, double safetyPct) {
        this.totalWindow = totalWindow;
        this.systemPct = systemPct;
        this.toolPct = toolPct;
        this.outputPct = outputPct;
        this.safetyPct = safetyPct;
    }

    public static TokenBudget newBudget(long totalWindow) {
        if (totalWindow <= 0) throw new IllegalArgumentException("token budget total window must be greater than zero");
        return new TokenBudget(totalWindow, 0.1, 0.05, 0.1, 0.1);
    }

    public static TokenBudget defaults() { return new TokenBudget(128_000, 0.1, 0.05, 0.1, 0.1); }
    public long totalWindow() { return totalWindow; }
    public TokenBudget withAllocations(double system, double tool, double output, double safety) {
        for (double value : new double[] {system, tool, output, safety}) {
            if (!Double.isFinite(value) || value < 0 || value > 1) throw new IllegalArgumentException("token budget allocation must be finite and in [0, 1]");
        }
        if (system + tool + output + safety > 1) throw new IllegalArgumentException("token budget allocations exceed 1.0");
        return new TokenBudget(totalWindow, system, tool, output, safety);
    }
    public long systemPromptBudget() { return Math.round(totalWindow * systemPct); }
    public long toolDefinitionsBudget() { return Math.round(totalWindow * toolPct); }
    public long outputBudget() { return Math.round(totalWindow * outputPct); }
    public long safetyBudget() { return Math.round(totalWindow * safetyPct); }
    public long conversationBudget() { return Math.round(totalWindow * Math.max(0, 1 - systemPct - toolPct - outputPct - safetyPct)); }
    public TokenAllocation allocate(long system, long tools, long conversation) {
        long effective = Math.max(0, totalWindow - outputBudget() - safetyBudget() - system - tools);
        return new TokenAllocation(system <= systemPromptBudget(), tools <= toolDefinitionsBudget(),
                conversation <= effective, outputBudget() > 0, Math.max(0, conversation - effective),
                (double) (system + tools + conversation) / totalWindow * 100);
    }
    public BudgetReport report(long system, long tools, long conversation, long estimatedOutput) {
        TokenAllocation allocation = allocate(system, tools, conversation);
        return new BudgetReport(totalWindow, system, systemPromptBudget(), tools, toolDefinitionsBudget(),
                conversation, conversationBudget(), estimatedOutput, outputBudget(), allocation.usagePct(), allocation.needsCompression());
    }
}
