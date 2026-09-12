package com.echoagent.sdk;

import java.util.ArrayList;
import java.util.List;

/** Prompt section diagnostics projected without owning prompt compilation. */
public final class PromptDiagnostics {
    public record Section(String id, String source) {}

    private final List<Section> sections = new ArrayList<>();

    public List<Section> sections() { return List.copyOf(sections); }

    public void record(String id, String source) { sections.add(new Section(id, source)); }

    public long count(String id) { return sections.stream().filter(section -> section.id().equals(id)).count(); }
}
