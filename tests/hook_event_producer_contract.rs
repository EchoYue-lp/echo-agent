use echo_agent::skills::hooks::HookEvent;
use std::collections::HashMap;

fn expected_producer(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PermissionDenied
        | HookEvent::Notification
        | HookEvent::ConfigChange
        | HookEvent::SkillLifecycleTransition
        | HookEvent::SkillPatchApplied
        | HookEvent::SkillMergeApplied
        | HookEvent::RulePromoted => "no-producer",
        HookEvent::TaskCreated
        | HookEvent::TaskStarted
        | HookEvent::TaskCompleted
        | HookEvent::PostMemoryWrite
        | HookEvent::MemoryLayerChange
        | HookEvent::SkillCandidateDetected
        | HookEvent::SkillHealthCheck => "host-owned",
        HookEvent::PreToolUse
        | HookEvent::PostToolUse
        | HookEvent::PostToolUseFailure
        | HookEvent::PermissionRequest
        | HookEvent::SessionStart
        | HookEvent::SessionEnd
        | HookEvent::Stop
        | HookEvent::UserPromptSubmit
        | HookEvent::PreCompact
        | HookEvent::PostCompact
        | HookEvent::InstructionsLoaded
        | HookEvent::PostToolBatch
        | HookEvent::SubagentStart
        | HookEvent::SubagentStop
        | HookEvent::StopFailure
        | HookEvent::PluginLoaded
        | HookEvent::PluginDisabled => "framework-auto",
    }
}

fn documented_producers(document: &str) -> HashMap<String, String> {
    let mut rows = HashMap::new();
    for line in document.lines().filter(|line| line.starts_with("| `")) {
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        let Some(name) = cells.get(1).and_then(|cell| cell.strip_prefix('`')) else {
            continue;
        };
        let Some(name) = name.strip_suffix('`') else {
            continue;
        };
        if !HookEvent::ALL.iter().any(|event| event.as_str() == name) {
            continue;
        }
        let Some(status) = cells.get(3).and_then(|cell| cell.strip_prefix('`')) else {
            continue;
        };
        let Some(status) = status.strip_suffix('`') else {
            continue;
        };
        assert!(
            rows.insert(name.to_string(), status.to_string()).is_none(),
            "duplicate producer row for {name}"
        );
    }
    rows
}

#[test]
fn bilingual_hook_producer_matrices_cover_the_entire_catalog() {
    let english = documented_producers(include_str!("../docs/en/23-hooks.md"));
    let chinese = documented_producers(include_str!("../docs/zh/23-hooks.md"));
    assert_eq!(HookEvent::ALL.len(), 31);
    assert_eq!(english.len(), HookEvent::ALL.len());
    assert_eq!(chinese.len(), HookEvent::ALL.len());
    for event in HookEvent::ALL {
        let name = event.as_str();
        assert_eq!(
            english.get(name).map(String::as_str),
            Some(expected_producer(*event)),
            "English producer classification for {name}"
        );
        assert_eq!(
            chinese.get(name).map(String::as_str),
            Some(expected_producer(*event)),
            "Chinese producer classification for {name}"
        );
    }
}
