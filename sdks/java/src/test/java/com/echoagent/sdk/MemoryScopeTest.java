package com.echoagent.sdk;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class MemoryScopeTest {
    @Test
    void preservesRustOrderNamesAndPersistence() {
        assertEquals(java.util.List.of(
                MemoryScope.USER, MemoryScope.PROJECT, MemoryScope.REPO,
                MemoryScope.TASK, MemoryScope.SESSION, MemoryScope.RUN), MemoryScope.all());
        assertEquals("project", MemoryScope.PROJECT.wireName());
        assertEquals(5, MemoryScope.RUN.priority());
        assertTrue(MemoryScope.USER.isPersistent());
        assertFalse(MemoryScope.TASK.isPersistent());
    }

    @Test
    void parserPreservesAliasesAndUnknownValues() {
        assertEquals(MemoryScope.PROJECT, MemoryScope.parse(" proj "));
        assertEquals(MemoryScope.SESSION, MemoryScope.parse("SESS"));
        assertNull(MemoryScope.parse("unknown"));
        assertNull(MemoryScope.parse(null));
    }
}
