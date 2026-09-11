package com.echoagent.sdk;

import java.util.List;

/** A2A skill description value. */
public final class AgentSkill {
    private final String id;
    private final String name;
    private final String description;
    private final List<String> examples;
    private final List<String> inputModes;
    private final List<String> outputModes;
    private final List<String> tags;

    private AgentSkill(String name, String description, List<String> examples, List<String> tags) {
        if (name == null || description == null) throw new IllegalArgumentException("skill name and description must not be null");
        this.id = name;
        this.name = name;
        this.description = description;
        this.examples = List.copyOf(examples == null ? List.of() : examples);
        this.inputModes = List.of();
        this.outputModes = List.of();
        this.tags = List.copyOf(tags == null ? List.of() : tags);
    }

    public static AgentSkill newSkill(String name, String description) {
        return new AgentSkill(name, description, List.of(), List.of());
    }
    public AgentSkill withExamples(List<String> values) {
        return new AgentSkill(name, description, textList(values, "examples"), tags);
    }
    public AgentSkill withTags(List<String> values) {
        return new AgentSkill(name, description, examples, textList(values, "tags"));
    }
    public String id() { return id; }
    public String name() { return name; }
    public String description() { return description; }
    public List<String> examples() { return examples; }
    public List<String> inputModes() { return inputModes; }
    public List<String> outputModes() { return outputModes; }
    public List<String> tags() { return tags; }

    private static List<String> textList(List<String> values, String field) {
        if (values == null || values.stream().anyMatch(value -> value == null)) {
            throw new IllegalArgumentException(field + " must be a non-null string list");
        }
        return List.copyOf(values);
    }
}
