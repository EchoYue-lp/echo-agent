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
const MAX_USIZE_64 = (1n << 64n) - 1n;

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

export type A2APart =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "file"; readonly mimeType: string; readonly data: string };

export class A2AMessage {
  public readonly role: string;
  public readonly parts: readonly A2APart[];

  private constructor(role: string, parts: readonly A2APart[]) {
    this.role = role;
    this.parts = Object.freeze(parts.map((part) => Object.freeze({ ...part })));
    Object.freeze(this);
  }

  public static userText(text: string): A2AMessage {
    if (typeof text !== "string") throw new TypeError("message text must be text");
    return new A2AMessage("user", [{ type: "text", text }]);
  }

  public static agentText(text: string): A2AMessage {
    if (typeof text !== "string") throw new TypeError("message text must be text");
    return new A2AMessage("agent", [{ type: "text", text }]);
  }

  public textContent(): string {
    return this.parts
      .filter((part): part is Extract<A2APart, { readonly type: "text" }> => part.type === "text")
      .map((part) => part.text)
      .join("\n");
  }
}

export class A2AArtifact {
  public readonly name?: string;
  public readonly index?: bigint;
  public readonly parts: readonly A2APart[];
  public readonly append: boolean;

  private constructor(parts: readonly A2APart[], name?: string, index?: bigint, append = false) {
    if (!Array.isArray(parts)) throw new TypeError("artifact parts must be an array");
    if (name !== undefined) validateText(name, "artifact name");
    if (index !== undefined && (index < 0n || index > MAX_USIZE_64)) {
      throw new TypeError("artifact index must fit Rust usize");
    }
    if (typeof append !== "boolean") throw new TypeError("artifact append must be boolean");
    this.name = name;
    this.index = index;
    this.parts = Object.freeze(parts.map((part) => freezePart(part)));
    this.append = append;
    Object.freeze(this);
  }

  public static new(
    parts: readonly A2APart[],
    options: { readonly name?: string; readonly index?: number | bigint; readonly append?: boolean } = {},
  ): A2AArtifact {
    return new A2AArtifact(parts, options.name, normalizeArtifactIndex(options.index), options.append ?? false);
  }
}

export class A2AError {
  public readonly code: number;
  public readonly message: string;

  private constructor(code: number, message: string) {
    if (!Number.isInteger(code) || code < -2147483648 || code > 2147483647) {
      throw new TypeError("A2A error code must be an i32");
    }
    validateText(message, "A2A error message");
    this.code = code;
    this.message = message;
    Object.freeze(this);
  }

  public static new(code: number, message: string): A2AError {
    return new A2AError(code, message);
  }
}

export class TaskStatusUpdateEvent {
  public readonly taskId: string;
  public readonly status: A2ATaskStatus;
  public readonly isFinal: boolean;

  private constructor(taskId: string, status: A2ATaskStatus, isFinal: boolean) {
    validateText(taskId, "task id");
    if (!(status instanceof A2ATaskStatus)) throw new TypeError("status must be an A2ATaskStatus");
    if (typeof isFinal !== "boolean") throw new TypeError("final must be boolean");
    this.taskId = taskId;
    this.status = status;
    this.isFinal = isFinal;
    Object.freeze(this);
  }

  public static new(taskId: string, status: A2ATaskStatus, isFinal = false): TaskStatusUpdateEvent {
    return new TaskStatusUpdateEvent(taskId, status, isFinal);
  }
}

export class TaskArtifactUpdateEvent {
  public readonly taskId: string;
  public readonly artifact: A2AArtifact;
  public readonly isFinal: boolean;

  private constructor(taskId: string, artifact: A2AArtifact, isFinal: boolean) {
    validateText(taskId, "task id");
    if (!(artifact instanceof A2AArtifact)) throw new TypeError("artifact must be an A2AArtifact");
    if (typeof isFinal !== "boolean") throw new TypeError("final must be boolean");
    this.taskId = taskId;
    this.artifact = artifact;
    this.isFinal = isFinal;
    Object.freeze(this);
  }

  public static new(taskId: string, artifact: A2AArtifact, isFinal = false): TaskArtifactUpdateEvent {
    return new TaskArtifactUpdateEvent(taskId, artifact, isFinal);
  }
}

export type A2AStreamEvent =
  | { readonly type: "status"; readonly event: TaskStatusUpdateEvent }
  | { readonly type: "artifact"; readonly event: TaskArtifactUpdateEvent };

export class A2AStreamResponse {
  public readonly jsonrpc: string;
  public readonly id: string;
  public readonly result?: A2AStreamEvent;
  public readonly error?: A2AError;

  private constructor(id: string, result?: A2AStreamEvent, error?: A2AError) {
    validateText(id, "stream response id");
    let frozenResult: A2AStreamEvent | undefined;
    if (result) {
      if (result.type === "status" && !(result.event instanceof TaskStatusUpdateEvent)) {
        throw new TypeError("invalid A2A status event");
      }
      if (result.type === "artifact" && !(result.event instanceof TaskArtifactUpdateEvent)) {
        throw new TypeError("invalid A2A artifact event");
      }
      if (result.type !== "status" && result.type !== "artifact") {
        throw new TypeError("invalid A2A stream event");
      }
      frozenResult = result.type === "status"
        ? Object.freeze({ type: "status", event: result.event })
        : Object.freeze({ type: "artifact", event: result.event });
    }
    if (error !== undefined && !(error instanceof A2AError)) throw new TypeError("error must be an A2AError");
    this.jsonrpc = "2.0";
    this.id = id;
    this.result = frozenResult;
    this.error = error;
    Object.freeze(this);
  }

  public static new(id: string, result?: A2AStreamEvent, error?: A2AError): A2AStreamResponse {
    return new A2AStreamResponse(id, result, error);
  }
}

export class A2ATaskParams {
  public readonly id?: string;
  public readonly sessionId?: string;
  public readonly message: A2AMessage;

  private constructor(message: A2AMessage, id?: string, sessionId?: string) {
    if (!(message instanceof A2AMessage)) throw new TypeError("message must be an A2AMessage");
    if (id !== undefined) validateText(id, "task id");
    if (sessionId !== undefined) validateText(sessionId, "session id");
    this.id = id;
    this.sessionId = sessionId;
    this.message = message;
    Object.freeze(this);
  }

  public static new(message: A2AMessage, id?: string, sessionId?: string): A2ATaskParams {
    return new A2ATaskParams(message, id, sessionId);
  }
}

export class A2ATaskRequest {
  public readonly jsonrpc = "2.0";
  public readonly id: string;
  public readonly method: string;
  public readonly params: A2ATaskParams;

  private constructor(id: string, method: string, params: A2ATaskParams) {
    validateText(id, "request id");
    validateText(method, "request method");
    if (!(params instanceof A2ATaskParams)) throw new TypeError("params must be A2ATaskParams");
    this.id = id;
    this.method = method;
    this.params = params;
    Object.freeze(this);
  }

  public static new(id: string, method: string, params: A2ATaskParams): A2ATaskRequest {
    return new A2ATaskRequest(id, method, params);
  }
}

export class A2ATask {
  public readonly id: string;
  public readonly sessionId?: string;
  public readonly status: A2ATaskStatus;
  public readonly history: readonly A2AMessage[];
  public readonly artifacts: readonly A2AArtifact[];

  private constructor(
    id: string,
    status: A2ATaskStatus,
    sessionId?: string,
    history: readonly A2AMessage[] = [],
    artifacts: readonly A2AArtifact[] = [],
  ) {
    validateText(id, "task id");
    if (sessionId !== undefined) validateText(sessionId, "session id");
    if (!(status instanceof A2ATaskStatus)) throw new TypeError("status must be an A2ATaskStatus");
    if (!Array.isArray(history) || history.some((value) => !(value instanceof A2AMessage))) {
      throw new TypeError("history must contain A2AMessage values");
    }
    if (!Array.isArray(artifacts) || artifacts.some((value) => !(value instanceof A2AArtifact))) {
      throw new TypeError("artifacts must contain A2AArtifact values");
    }
    this.id = id;
    this.sessionId = sessionId;
    this.status = status;
    this.history = Object.freeze([...history]);
    this.artifacts = Object.freeze([...artifacts]);
    Object.freeze(this);
  }

  public static new(
    id: string,
    status: A2ATaskStatus,
    sessionId?: string,
    history: readonly A2AMessage[] = [],
    artifacts: readonly A2AArtifact[] = [],
  ): A2ATask {
    return new A2ATask(id, status, sessionId, history, artifacts);
  }
}

export class A2ATaskResponse {
  public readonly jsonrpc = "2.0";
  public readonly id?: string;
  public readonly result?: A2ATask;
  public readonly error?: A2AError;

  private constructor(id?: string, result?: A2ATask, error?: A2AError) {
    if (id !== undefined) validateText(id, "response id");
    if (result !== undefined && !(result instanceof A2ATask)) throw new TypeError("result must be an A2ATask");
    if (error !== undefined && !(error instanceof A2AError)) throw new TypeError("error must be an A2AError");
    this.id = id;
    this.result = result;
    this.error = error;
    Object.freeze(this);
  }

  public static new(id?: string, result?: A2ATask, error?: A2AError): A2ATaskResponse {
    return new A2ATaskResponse(id, result, error);
  }
}

export class A2ATaskStatus {
  public readonly state: TaskState;
  public readonly message?: A2AMessage;
  public readonly timestamp: string;

  private constructor(state: TaskState, message?: A2AMessage) {
    validateTaskState(state);
    this.state = state;
    this.message = message;
    this.timestamp = new Date().toISOString();
    Object.freeze(this);
  }

  public static new(state: TaskState): A2ATaskStatus {
    return new A2ATaskStatus(state);
  }

  public static withMessage(state: TaskState, message: A2AMessage): A2ATaskStatus {
    if (!(message instanceof A2AMessage)) throw new TypeError("A2A task status message must be an A2AMessage");
    return new A2ATaskStatus(state, message);
  }
}

export class AgentProvider {
  public readonly organization: string;
  public readonly url?: string;

  private constructor(organization: string, url?: string) {
    this.organization = organization;
    this.url = url;
    Object.freeze(this);
  }

  public static new(organization: string): AgentProvider {
    if (typeof organization !== "string") throw new TypeError("organization must be text");
    return new AgentProvider(organization);
  }

  public withUrl(url: string): AgentProvider {
    if (typeof url !== "string") throw new TypeError("provider url must be text");
    return new AgentProvider(this.organization, url);
  }
}

export class AgentSkill {
  public readonly id: string;
  public readonly name: string;
  public readonly description?: string;
  public readonly examples: readonly string[];
  public readonly inputModes: readonly string[];
  public readonly outputModes: readonly string[];
  public readonly tags: readonly string[];

  private constructor(
    name: string,
    description: string,
    examples: readonly string[] = [],
    tags: readonly string[] = [],
  ) {
    this.id = name;
    this.name = name;
    this.description = description;
    this.examples = Object.freeze([...examples]);
    this.inputModes = Object.freeze([]);
    this.outputModes = Object.freeze([]);
    this.tags = Object.freeze([...tags]);
    Object.freeze(this);
  }

  public static new(name: string, description: string): AgentSkill {
    if (typeof name !== "string" || typeof description !== "string") {
      throw new TypeError("skill name and description must be text");
    }
    return new AgentSkill(name, description);
  }

  public withExamples(examples: readonly string[]): AgentSkill {
    return this.copy({ examples: textList(examples, "examples") });
  }

  public withTags(tags: readonly string[]): AgentSkill {
    return this.copy({ tags: textList(tags, "tags") });
  }

  private copy(changes: Partial<AgentSkill>): AgentSkill {
    return new AgentSkill(
      this.name,
      this.description ?? "",
      changes.examples ?? this.examples,
      changes.tags ?? this.tags,
    );
  }
}

export type AuthenticationScheme = Readonly<{
  scheme: string;
  config: Readonly<Record<string, unknown>>;
}>;

export type AgentAuthentication = Readonly<{
  schemes: readonly AuthenticationScheme[];
}>;

export type AgentCapabilities = Readonly<{
  streaming: boolean;
  pushNotifications: boolean;
  stateTransitionHistory: boolean;
}>;

type AgentCardInput = {
  name: string;
  url: string;
  description?: string;
  version?: string;
  provider?: AgentProvider;
  skills: readonly AgentSkill[];
  defaultInputModes: readonly string[];
  defaultOutputModes: readonly string[];
  authentication?: AgentAuthentication;
  capabilities: AgentCapabilities;
};

export class AgentCard {
  public readonly name: string;
  public readonly description?: string;
  public readonly url: string;
  public readonly version?: string;
  public readonly provider?: AgentProvider;
  public readonly skills: readonly AgentSkill[];
  public readonly defaultInputModes: readonly string[];
  public readonly defaultOutputModes: readonly string[];
  public readonly authentication?: AgentAuthentication;
  public readonly capabilities: AgentCapabilities;

  private constructor(input: AgentCardInput) {
    this.name = input.name;
    this.description = input.description;
    this.url = input.url;
    this.version = input.version;
    this.provider = input.provider;
    this.skills = Object.freeze([...input.skills]);
    this.defaultInputModes = Object.freeze([...input.defaultInputModes]);
    this.defaultOutputModes = Object.freeze([...input.defaultOutputModes]);
    this.authentication = input.authentication
      ? Object.freeze({
          schemes: Object.freeze(input.authentication.schemes.map((scheme) =>
            Object.freeze({ scheme: scheme.scheme, config: Object.freeze({ ...scheme.config }) }),
          )),
        })
      : undefined;
    this.capabilities = Object.freeze({ ...input.capabilities });
    Object.freeze(this);
  }

  public static builder(name: string, url: string): AgentCardBuilder {
    validateText(name, "agent name");
    validateText(url, "agent url");
    return new AgentCardBuilder(name, url);
  }

  /** @internal */
  public static fromInput(input: AgentCardInput): AgentCard {
    return new AgentCard(input);
  }

}

export class AgentCardBuilder {
  private descriptionValue?: string;
  private versionValue?: string;
  private providerValue?: AgentProvider;
  private readonly skillsValue: AgentSkill[] = [];
  private inputModesValue: string[] = ["text/plain"];
  private outputModesValue: string[] = ["text/plain"];
  private authenticationValue?: AgentAuthentication;
  private streamingValue = false;
  private pushNotificationsValue = false;

  private readonly nameValue: string;
  private readonly urlValue: string;

  constructor(nameValue: string, urlValue: string) {
    validateText(nameValue, "agent name");
    validateText(urlValue, "agent url");
    this.nameValue = nameValue;
    this.urlValue = urlValue;
  }

  public description(value: string): AgentCardBuilder {
    validateText(value, "agent description");
    this.descriptionValue = value;
    return this;
  }

  public version(value: string): AgentCardBuilder {
    validateText(value, "agent version");
    this.versionValue = value;
    return this;
  }

  public provider(value: AgentProvider): AgentCardBuilder {
    if (!(value instanceof AgentProvider)) throw new TypeError("provider must be an AgentProvider");
    this.providerValue = value;
    return this;
  }

  public skill(value: AgentSkill): AgentCardBuilder {
    if (!(value instanceof AgentSkill)) throw new TypeError("skill must be an AgentSkill");
    this.skillsValue.push(value);
    return this;
  }

  public skills(values: readonly AgentSkill[]): AgentCardBuilder {
    if (!Array.isArray(values) || values.some((value) => !(value instanceof AgentSkill))) {
      throw new TypeError("skills must be an AgentSkill array");
    }
    this.skillsValue.push(...values);
    return this;
  }

  public inputModes(values: readonly string[]): AgentCardBuilder {
    this.inputModesValue = [...textList(values, "input modes")];
    return this;
  }

  public outputModes(values: readonly string[]): AgentCardBuilder {
    this.outputModesValue = [...textList(values, "output modes")];
    return this;
  }

  public authentication(value: AgentAuthentication): AgentCardBuilder {
    if (!value || !Array.isArray(value.schemes)) throw new TypeError("invalid agent authentication");
    this.authenticationValue = value;
    return this;
  }

  public streaming(): AgentCardBuilder {
    this.streamingValue = true;
    return this;
  }

  public pushNotifications(): AgentCardBuilder {
    this.pushNotificationsValue = true;
    return this;
  }

  public build(): AgentCard {
    return AgentCard.fromInput({
      name: this.nameValue,
      url: this.urlValue,
      description: this.descriptionValue,
      version: this.versionValue,
      provider: this.providerValue,
      skills: this.skillsValue,
      defaultInputModes: this.inputModesValue,
      defaultOutputModes: this.outputModesValue,
      authentication: this.authenticationValue,
      capabilities: {
        streaming: this.streamingValue,
        pushNotifications: this.pushNotificationsValue,
        stateTransitionHistory: false,
      },
    });
  }
}

function validateTaskState(state: string): asserts state is TaskState {
  if (!TASK_STATES.has(state)) throw new TypeError(`unknown A2A task state: ${state}`);
}

function textList(values: readonly string[], field: string): readonly string[] {
  if (!Array.isArray(values) || values.some((value) => typeof value !== "string")) {
    throw new TypeError(`${field} must be a string array`);
  }
  return [...values];
}

function validateText(value: unknown, field: string): asserts value is string {
  if (typeof value !== "string") throw new TypeError(`${field} must be text`);
}

function freezePart(part: A2APart): A2APart {
  if (!part || (part.type !== "text" && part.type !== "file")) {
    throw new TypeError("artifact parts must be valid A2A parts");
  }
  if (part.type === "text") validateText(part.text, "artifact text");
  if (part.type === "file") {
    validateText(part.mimeType, "artifact mime type");
    validateText(part.data, "artifact data");
  }
  return Object.freeze({ ...part });
}

function normalizeArtifactIndex(value: number | bigint | undefined): bigint | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "bigint") {
    if (value < 0n || value > MAX_USIZE_64) throw new TypeError("artifact index must fit Rust usize");
    return value;
  }
  if (!Number.isSafeInteger(value) || value < 0) throw new TypeError("artifact index must be a non-negative safe integer");
  return BigInt(value);
}
