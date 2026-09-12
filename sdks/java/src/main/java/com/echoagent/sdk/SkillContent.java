package com.echoagent.sdk;

import java.util.List;

/** Activated Skill content projection without loading or executing resources. */
public record SkillContent(
        String name,
        String skillDir,
        String instructions,
        List<String> allowedTools,
        List<SkillResourceEntry> resources) {
    public record SkillResourceEntry(String kind, String relativePath) {}

    public SkillContent {
        allowedTools = List.copyOf(allowedTools);
        resources = List.copyOf(resources);
    }

    public String toPromptBlock() {
        var block = new StringBuilder("<skill_content name=\"").append(name).append("\">\n")
                .append(instructions.trim()).append("\n\nSkill directory: ").append(skillDir)
                .append("\nRelative paths in this skill are relative to the skill directory.");
        if (!allowedTools.isEmpty()) {
            block.append("\n\n<allowed_tools>\nThis skill declares the following preferred/allowed tools: ")
                    .append(String.join(", ", allowedTools))
                    .append("\nRuntime enforcement currently applies to the built-in skill tools such as read_skill_resource and run_skill_script.\n</allowed_tools>");
        }
        if (!resources.isEmpty()) {
            block.append("\n\n<skill_resources>");
            for (var resource : resources) {
                block.append("\n  <file kind=\"").append(resource.kind()).append("\">")
                        .append(resource.relativePath()).append("</file>");
            }
            block.append("\n</skill_resources>");
        }
        return block.append("\n</skill_content>").toString();
    }
}
