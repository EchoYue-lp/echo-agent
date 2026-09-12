package com.echoagent.sdk;

/** Durable Subagent command identities without owning live-control state. */
public record SubagentCommandIdentity(
        String runId, String taskId, String executionId, long planRevision, long attempt, String commandId) {
    public SubagentCommandIdentity(String runId, String taskId, String executionId,
            long planRevision, long attempt, String commandId) {
        this.runId = runId;
        this.taskId = taskId;
        this.executionId = executionId;
        this.planRevision = planRevision;
        this.attempt = attempt;
        this.commandId = commandId;
        validate();
    }

    public void validate() {
        if (runId == null || runId.isBlank()) throw new IllegalArgumentException("invalid identity: run_id");
        if (planRevision <= 0) throw new IllegalArgumentException("invalid identity: plan_revision");
        if (commandId == null || commandId.isBlank()) throw new IllegalArgumentException("invalid identity: command_id");
        attemptIdentity();
    }

    public SubagentAttemptIdentity attemptIdentity() {
        return new SubagentAttemptIdentity(taskId, executionId, attempt);
    }

    public record SubagentAttemptIdentity(String taskId, String executionId, long attempt) {
        public SubagentAttemptIdentity(String taskId, String executionId, long attempt) {
            this.taskId = taskId;
            this.executionId = executionId;
            this.attempt = attempt;
            if (taskId == null || taskId.isBlank()) throw new IllegalArgumentException("invalid identity: task_id");
            if (executionId == null || executionId.isBlank()) throw new IllegalArgumentException("invalid identity: execution_id");
            if (attempt < 0 || attempt > 0xFFFFFFFFL) throw new IllegalArgumentException("invalid identity: attempt");
        }
    }
}
