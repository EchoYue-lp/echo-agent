package com.echoagent.sdk;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class MemoryPolicyValuesTest {
    @Test
    void memoryTypePreservesStabilityAndEligibility() {
        assertEquals(0.85, MemoryType.USER_PREFERENCE.defaultStability());
        assertEquals(0.1, MemoryType.DEPRECATED_NOTE.defaultStability());
        assertTrue(MemoryType.PROJECT_FACT.isRuleEligible());
        assertFalse(MemoryType.WORKFLOW_PATTERN.isRuleEligible());
        assertTrue(MemoryType.DEBUGGING_LESSON.isSkillEligible());
        assertFalse(MemoryType.USER_PREFERENCE.isSkillEligible());
    }

    @Test
    void memorySourcePreservesConfidenceAndRecallWeight() {
        assertEquals(1.0, MemorySource.EXPLICIT_SAVE.defaultConfidence());
        assertEquals(0.6, MemorySource.AUTO_EXTRACTED.defaultConfidence());
        assertEquals(0.9, MemorySource.USER_CORRECTION.defaultRecallWeight());
        assertEquals(0.4, MemorySource.L3_PROMOTION.defaultRecallWeight());
    }
}
