package com.echoagent.sdk;

/** Human-readable token budget report projected without model execution. */
public record BudgetReport(long totalWindow, long systemPrompt, long systemPromptBudget,
        long toolDefinitions, long toolDefinitionsBudget, long conversation, long conversationBudget,
        long estimatedOutput, long outputBudget, double usagePct, boolean needsCompression) {}
