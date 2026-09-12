package com.echoagent.sdk;

/** Token allocation result projected without model execution. */
public record TokenAllocation(boolean systemFits, boolean toolDefsFit, boolean conversationFits,
        boolean outputFits, long conversationExcess, double usagePct) {
    public boolean ok() { return systemFits && toolDefsFit && conversationFits && outputFits; }
    public boolean needsCompression() { return conversationExcess > 0; }
}
