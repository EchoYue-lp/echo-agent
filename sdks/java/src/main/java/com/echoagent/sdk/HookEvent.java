package com.echoagent.sdk;

import java.util.EnumSet;
import java.util.List;
import java.util.Optional;
import java.util.Set;

/** Hook event names and matcher semantics projected without hook execution. */
public enum HookEvent {
    PRE_TOOL_USE("PreToolUse"), POST_TOOL_USE("PostToolUse"),
    POST_TOOL_USE_FAILURE("PostToolUseFailure"), PERMISSION_REQUEST("PermissionRequest"),
    PERMISSION_DENIED("PermissionDenied"), SESSION_START("SessionStart"),
    SESSION_END("SessionEnd"), STOP("Stop"), NOTIFICATION("Notification"),
    USER_PROMPT_SUBMIT("UserPromptSubmit"), PRE_COMPACT("PreCompact"),
    POST_COMPACT("PostCompact"), CONFIG_CHANGE("ConfigChange"),
    INSTRUCTIONS_LOADED("InstructionsLoaded"), POST_TOOL_BATCH("PostToolBatch"),
    SUBAGENT_START("SubagentStart"), SUBAGENT_STOP("SubagentStop"),
    TASK_CREATED("TaskCreated"), TASK_STARTED("TaskStarted"),
    TASK_COMPLETED("TaskCompleted"), STOP_FAILURE("StopFailure"),
    PLUGIN_LOADED("PluginLoaded"), PLUGIN_DISABLED("PluginDisabled"),
    POST_MEMORY_WRITE("PostMemoryWrite"), MEMORY_LAYER_CHANGE("MemoryLayerChange"),
    SKILL_CANDIDATE_DETECTED("SkillCandidateDetected"),
    SKILL_LIFECYCLE_TRANSITION("SkillLifecycleTransition"),
    SKILL_HEALTH_CHECK("SkillHealthCheck"), SKILL_PATCH_APPLIED("SkillPatchApplied"),
    SKILL_MERGE_APPLIED("SkillMergeApplied"), RULE_PROMOTED("RulePromoted");

    public static final List<HookEvent> ALL = List.of(values());
    private static final Set<HookEvent> TOOL_EVENTS = EnumSet.of(
            PRE_TOOL_USE, POST_TOOL_USE, POST_TOOL_USE_FAILURE,
            PERMISSION_REQUEST, PERMISSION_DENIED);
    private static final Set<HookEvent> SUBAGENT_EVENTS = EnumSet.of(SUBAGENT_START, SUBAGENT_STOP);
    private static final Set<HookEvent> TASK_EVENTS = EnumSet.of(TASK_CREATED, TASK_STARTED, TASK_COMPLETED);
    private static final Set<HookEvent> EVOLUTION_EVENTS = EnumSet.of(
            POST_MEMORY_WRITE, MEMORY_LAYER_CHANGE, SKILL_CANDIDATE_DETECTED,
            SKILL_LIFECYCLE_TRANSITION, SKILL_HEALTH_CHECK, SKILL_PATCH_APPLIED,
            SKILL_MERGE_APPLIED, RULE_PROMOTED);

    private final String wireName;

    HookEvent(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    public static Optional<HookEvent> fromName(String name) {
        for (HookEvent event : values()) {
            if (event.wireName.equals(name)) return Optional.of(event);
        }
        return Optional.empty();
    }

    public HookEventCategory category() {
        if (TOOL_EVENTS.contains(this)) return HookEventCategory.TOOL;
        if (SUBAGENT_EVENTS.contains(this)) return HookEventCategory.SUBAGENT;
        if (TASK_EVENTS.contains(this)) return HookEventCategory.TASK;
        if (this == STOP_FAILURE) return HookEventCategory.ERROR;
        if (EVOLUTION_EVENTS.contains(this)) return HookEventCategory.EVOLUTION;
        return HookEventCategory.LIFECYCLE;
    }

    public boolean isToolEvent() { return TOOL_EVENTS.contains(this); }

    public boolean supportsMatcher() { return true; }

    @Override
    public String toString() { return wireName; }
}
