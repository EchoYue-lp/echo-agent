/** Closed A2A task-state values exposed by the Rust facade. */
export const TaskState = Object.freeze({
  Submitted: "submitted",
  Working: "working",
  InputRequired: "input-required",
  Completed: "completed",
  Failed: "failed",
  Canceled: "canceled",
} as const);

export type TaskState = (typeof TaskState)[keyof typeof TaskState];

const TASK_STATES: ReadonlySet<string> = new Set(Object.values(TaskState));

export function taskStateIsTerminal(state: TaskState): boolean {
  validateTaskState(state);
  return state === TaskState.Completed || state === TaskState.Failed || state === TaskState.Canceled;
}

export function taskStateCanTransitionTo(current: TaskState, next: TaskState): boolean {
  validateTaskState(current);
  validateTaskState(next);
  if (taskStateIsTerminal(current)) return false;
  return (current === TaskState.Submitted
      && (next === TaskState.Working || next === TaskState.Canceled))
    || (current === TaskState.Working
      && (next === TaskState.Completed
        || next === TaskState.Failed
        || next === TaskState.InputRequired
        || next === TaskState.Canceled))
    || (current === TaskState.InputRequired
      && (next === TaskState.Working || next === TaskState.Canceled));
}

function validateTaskState(state: string): asserts state is TaskState {
  if (!TASK_STATES.has(state)) throw new TypeError(`unknown A2A task state: ${state}`);
}
