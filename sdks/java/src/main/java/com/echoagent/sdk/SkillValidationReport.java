package com.echoagent.sdk;

import java.util.List;

/** Skill validation report values without running validation. */
public record SkillValidationReport(String path, List<String> violations, List<String> warnings) {
    public SkillValidationReport {
        violations = List.copyOf(violations);
        warnings = List.copyOf(warnings);
    }

    public boolean isValid() { return violations.isEmpty(); }
}
