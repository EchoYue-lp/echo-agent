import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { Readable, Writable } from "node:stream";
import {
  client as createAcpClient,
  methods,
  ndJsonStream,
  type ClientApp,
  type ClientConnection,
} from "@agentclientprotocol/sdk";
import { FacadeCatalog } from "./catalog.js";
import { EchoAgentError } from "./errors.js";
import type {
  EchoAgentCapability,
  EventGap,
  ExtensionDescriptor,
  ExtensionKind,
  ExtensionInvocation,
  ExtensionInvokeCall,
  ExtensionResult,
  ExtensionOutcome,
  FacadeEvent,
  FacadeGap,
  FacadeStreamItem,
  LlmClientDescriptor,
  LlmClientExtensionHandler,
  LlmExtensionResult,
  LlmInvocation,
  CriticDescriptor,
  CriticExtensionHandler,
  CriticInvocation,
  CriticExtensionResult,
  ContextCompressorExtensionHandler,
  ContextCompressorExtensionResult,
  ContextCompressorInvocation,
  ContextCompressorDescriptor,
  AgentComponentDescriptor,
  AgentComponentExtensionHandler,
  AgentComponentExtensionResult,
  AgentComponentInvocation,
  AgentComponentStreamChunk,
  AgentComponentStreamComplete,
  ExecutionUsage,
  RunStartResult,
  RunGetResult,
  RunWaitResult,
  RunCancelResult,
  ReplayResult,
  StoreDescriptor,
  StoreExtensionHandler,
  StoreExtensionResult,
  StoreInvocation,
  ToolDescriptor,
  ToolExtensionHandler,
  ToolExtensionResult,
  ToolInvocation,
  TurnOutcomeClassification,
  WireEventEnvelope,
  WireHandle,
  WireValue,
  TurnStatus,
  TokenizerReference,
} from "./types.js";
import { fromWireValue, isWireHandle, isWireValue, toWireValue } from "./wire.js";

const DEFAULT_QUEUE_CAPACITY = 256;
const MAX_QUEUE_CAPACITY = 4096;
const DEFAULT_CLOSE_WAIT_MS = 5000;

export interface SpawnOptions {
  readonly hostCommand: string;
  readonly args?: readonly string[];
  readonly cwd?: string;
  readonly env?: NodeJS.ProcessEnv;
  readonly catalogPath?: string;
  readonly requiredFeatures?: readonly string[];
  readonly requiredCapabilities?: readonly string[];
  readonly contractDigest?: string;
  readonly sourceContractDigest?: string;
  readonly onStderr?: (chunk: string) => void;
}

export type ExtensionHandler = (call: unknown, signal: AbortSignal) => Promise<unknown> | unknown;
export type ExtensionInvokeOutcome = ExtensionOutcome<ExtensionResult>;

type QueueItem<T> = { readonly value: T } | { readonly error: unknown } | { readonly done: true };

class AsyncQueue<T> implements AsyncIterable<T> {
  private readonly capacity: number;
  private readonly items: QueueItem<T>[] = [];
  private readonly waiters: Array<(item: QueueItem<T>) => void> = [];
  private closed = false;

  constructor(capacity = DEFAULT_QUEUE_CAPACITY) {
    this.capacity = Math.max(1, Math.min(MAX_QUEUE_CAPACITY, Math.trunc(capacity)));
  }

  push(value: T): void {
    if (this.closed) return;
    if (this.waiters.length === 0 && this.items.length >= this.capacity) {
      this.fail(new EchoAgentError(
        "backpressure",
        `SDK stream queue exceeded its bound of ${this.capacity} items`,
        "after_delay",
      ));
      return;
    }
    this.shift({ value });
  }

  fail(error: unknown): void {
    if (this.closed) return;
    this.closed = true;
    this.shift({ error });
    while (this.waiters.length > 0) this.shift({ done: true });
  }

  end(): void {
    if (this.closed) return;
    this.closed = true;
    while (this.waiters.length > 0) this.shift({ done: true });
  }

  private shift(item: QueueItem<T>): void {
    const waiter = this.waiters.shift();
    if (waiter) waiter(item);
    else this.items.push(item);
  }

  private async nextItem(): Promise<QueueItem<T>> {
    const item = this.items.shift();
    if (item) return item;
    if (this.closed) return { done: true };
    return new Promise((resolve) => this.waiters.push(resolve));
  }

  async *[Symbol.asyncIterator](): AsyncIterator<T> {
    while (true) {
      const item = await this.nextItem();
      if ("done" in item) return;
      if ("error" in item) throw item.error;
      yield item.value;
    }
  }
}

interface StreamFeed {
  readonly queue: AsyncQueue<FacadeStreamItem>;
  handle?: WireHandle;
  lastEventSequence: bigint;
}

function boundedQueueCapacity(capability: EchoAgentCapability): number {
  const raw = capability.limits.max_outstanding_live_events
    ?? capability.limits.max_stream_buffer_events;
  if (typeof raw === "number" && Number.isSafeInteger(raw) && raw > 0) {
    return Math.min(MAX_QUEUE_CAPACITY, raw);
  }
  if (typeof raw === "string" && /^\d+$/u.test(raw)) {
    try {
      const parsed = BigInt(raw);
      if (parsed > 0n) return Number(parsed > BigInt(MAX_QUEUE_CAPACITY) ? MAX_QUEUE_CAPACITY : parsed);
    } catch {
      // A malformed advertisement is rejected by the Host capability check;
      // retain a bounded local default until that check completes.
    }
  }
  return DEFAULT_QUEUE_CAPACITY;
}

export class EchoAgentClient {
  readonly catalog: FacadeCatalog;
  readonly capability: EchoAgentCapability;
  private readonly app: ClientApp;
  private readonly connection: ClientConnection;
  private readonly process: ChildProcessWithoutNullStreams;
  private readonly extensionHandlers: Map<string, ExtensionHandler>;
  private readonly events: Map<string, StreamFeed>;
  private readonly updates: Map<string, AsyncQueue<unknown>>;
  private readonly queueCapacity: number;
  private closed = false;

  private constructor(
    app: ClientApp,
    connection: ClientConnection,
    child: ChildProcessWithoutNullStreams,
    catalog: FacadeCatalog,
    capability: EchoAgentCapability,
    extensionHandlers: Map<string, ExtensionHandler>,
    queueCapacity: number,
    events: Map<string, StreamFeed>,
    updates: Map<string, AsyncQueue<unknown>>,
  ) {
    this.app = app;
    this.connection = connection;
    this.process = child;
    this.catalog = catalog;
    this.capability = capability;
    this.extensionHandlers = extensionHandlers;
    this.queueCapacity = queueCapacity;
    this.events = events;
    this.updates = updates;
  }

  static async spawn(options: SpawnOptions): Promise<EchoAgentClient> {
    const child = spawn(options.hostCommand, [...(options.args ?? [])], {
      cwd: options.cwd,
      env: options.env ? { ...process.env, ...options.env } : process.env,
      stdio: ["pipe", "pipe", "pipe"],
    });
    if (!child.stdin || !child.stdout || !child.stderr) {
      child.kill();
      throw new EchoAgentError("transport_error", "Host did not expose all stdio pipes");
    }
    if (options.onStderr) {
      child.stderr.on("data", (chunk: Buffer) => options.onStderr?.(chunk.toString("utf8")));
    }
    await new Promise<void>((resolve, reject) => {
      const onSpawn = () => {
        child.off("error", onError);
        resolve();
      };
      const onError = (error: Error) => {
        child.off("spawn", onSpawn);
        reject(new EchoAgentError("transport_error", `Host failed to start: ${error.message}`));
      };
      child.once("spawn", onSpawn);
      child.once("error", onError);
    }).catch((error) => {
      child.kill();
      throw error;
    });
    const catalog = new FacadeCatalog(options.catalogPath);
    let queueCapacity = DEFAULT_QUEUE_CAPACITY;
    const eventFeeds = new Map<string, StreamFeed>();
    const updateQueues = new Map<string, AsyncQueue<unknown>>();
    const extensionHandlers = new Map<string, ExtensionHandler>();
    const invocationControllers = new Map<string, AbortController>();
    const app = createAcpClient({ name: "echo-agent-sdk-typescript" });
    const customApp = app as unknown as {
      onNotification(method: string, parser: (params: unknown) => unknown, handler: (context: { params: unknown }) => Promise<void> | void): ClientApp;
      onRequest(method: string, parser: (params: unknown) => unknown, handler: (context: { params: unknown; signal: AbortSignal }) => Promise<unknown> | unknown): ClientApp;
    };
    customApp.onNotification(methods.client.session.update, (params) => params, async ({ params }) => {
      const sessionId = readNonEmptyString(params, "sessionId");
      if (!sessionId) {
        failQueues(updateQueues, new EchoAgentError(
          "serialization_violation",
          "session/update notification is missing sessionId",
        ));
        return;
      }
      let queue = updateQueues.get(sessionId);
      if (!queue) {
        queue = new AsyncQueue<unknown>(queueCapacity);
        updateQueues.set(sessionId, queue);
      }
      queue.push(params);
    });
    customApp.onNotification("_echo_agent/event", (params) => params, async ({ params }) => {
      let event: FacadeEvent;
      try {
        event = decodeEvent(params);
      } catch (error) {
        failFeeds(eventFeeds, EchoAgentError.fromUnknown(error, "_echo_agent/event"));
        return;
      }
      const streamId = event.stream.id;
      let feed = eventFeeds.get(streamId);
      if (!feed) {
        feed = { queue: new AsyncQueue<FacadeStreamItem>(queueCapacity), lastEventSequence: 0n };
        eventFeeds.set(streamId, feed);
      }
      if (feed.handle && !sameHandle(feed.handle, event.stream)) {
        feed.queue.fail(new EchoAgentError("handle_mismatch", "event stream handle changed generation"));
        return;
      }
      feed.handle ??= event.stream;
      const sequence = parsePositiveSequence(event.envelope.sequence);
      if (sequence === feed.lastEventSequence) {
        return;
      }
      if (sequence < feed.lastEventSequence) {
        feed.queue.fail(new EchoAgentError(
          "serialization_violation",
          `event sequence ${event.envelope.sequence} is not strictly increasing`,
          "never",
          "_echo_agent/event",
        ));
        return;
      }
      feed.lastEventSequence = sequence;
      feed.queue.push(event);
      if (["final_answer", "cancelled", "error"].includes(event.envelope.payload.event_type)) {
        // The terminal event remains available to the consumer before the
        // iterator observes completion. Run snapshots remain authoritative.
        feed.queue.end();
      }
    });
    customApp.onNotification("_echo_agent/gap", (params) => params, async ({ params }) => {
      let gap: FacadeGap;
      try {
        gap = decodeGap(params);
      } catch (error) {
        failFeeds(eventFeeds, EchoAgentError.fromUnknown(error, "_echo_agent/gap"));
        return;
      }
      const streamId = gap.stream.id;
      let feed = eventFeeds.get(streamId);
      if (!feed) {
        feed = { queue: new AsyncQueue<FacadeStreamItem>(queueCapacity), lastEventSequence: 0n };
        eventFeeds.set(streamId, feed);
      }
      if (feed.handle && !sameHandle(feed.handle, gap.stream)) {
        feed.queue.fail(new EchoAgentError("handle_mismatch", "gap stream handle changed generation"));
        return;
      }
      feed.handle ??= gap.stream;
      feed.lastEventSequence = parsePositiveSequence(gap.gap.snapshot_watermark);
      feed.queue.push(gap);
    });
    customApp.onNotification("_echo_agent/extension/cancel", (params) => params, async ({ params }) => {
      const invocationId = typeof params === "object" && params !== null && "invocation_id" in params
        ? String((params as { invocation_id: unknown }).invocation_id)
        : "";
      invocationControllers.get(invocationId)?.abort();
    });
    customApp.onRequest("_echo_agent/extension/invoke", (params) => params, async ({ params, signal }) => {
      const call = params as { extension?: WireHandle; invocation_id?: string };
      const extensionId = call.extension?.id;
      const handler = extensionId ? extensionHandlers.get(extensionId) : undefined;
      if (!handler) {
        return {
          outcome: "error",
          error: {
            code: "invalid_value",
            message: "extension registration is not available in this SDK client",
            retryable: "never",
          },
        };
      }
      const controller = new AbortController();
      signal.addEventListener("abort", () => controller.abort(), { once: true });
      if (typeof call.invocation_id === "string") invocationControllers.set(call.invocation_id, controller);
      try {
        const result = await handler(params, controller.signal);
        return controller.signal.aborted ? cancelledOutcome() : normalizeOutcome(result);
      } catch (error) {
        const failure = extensionFailure(error);
        return {
          outcome: "error",
          error: {
            code: failure.code,
            message: failure.message,
            retryable: failure.retryable,
            operation: failure.operation,
            details: failure.details,
          },
        };
      } finally {
        if (typeof call.invocation_id === "string") invocationControllers.delete(call.invocation_id);
      }
    });
    const stream = ndJsonStream(
      Writable.toWeb(child.stdin) as WritableStream<Uint8Array>,
      Readable.toWeb(child.stdout) as ReadableStream<Uint8Array>,
    );
    const connection = app.connect(stream);
    const hello = {
      extension_protocol_version: 1,
      contract_digest: options.contractDigest ?? catalog.contractDigest,
      source_contract_digest: options.sourceContractDigest ?? catalog.sourceContractDigest,
      required_features: [...(options.requiredFeatures ?? [])].sort(),
      required_capabilities: [...(options.requiredCapabilities ?? [])].sort(),
    };
    try {
      const initialized = await connection.agent.request("initialize", {
        protocolVersion: 1,
        clientCapabilities: {
          fs: { readTextFile: false, writeTextFile: false },
          terminal: false,
          _meta: { echo_agent: hello },
        },
      });
      const capability = (initialized as unknown as { agentCapabilities?: { _meta?: { echo_agent?: EchoAgentCapability } } })
        .agentCapabilities?._meta?.echo_agent;
      if (!capability) throw new EchoAgentError("extension_capability_mismatch", "Host did not advertise echo-agent extension");
      validateCapability(capability, options);
      queueCapacity = boundedQueueCapacity(capability);
      const result = new EchoAgentClient(
        app, connection, child, catalog, capability, extensionHandlers, queueCapacity,
        eventFeeds, updateQueues,
      );
      const onHostExit = () => {
        const error = new EchoAgentError("host_exited", "echo-agent SDK Host exited before the client closed");
        for (const feed of result.events.values()) {
          if (result.closed) feed.queue.end();
          else feed.queue.fail(error);
        }
        for (const queue of result.updates.values()) {
          if (result.closed) queue.end();
          else queue.fail(error);
        }
      };
      child.once("exit", onHostExit);
      child.once("error", (error) => {
        if (result.closed) return;
        const failure = new EchoAgentError("transport_error", `Host process failed: ${error.message}`);
        failFeeds(result.events, failure);
        failQueues(result.updates, failure);
      });
      return result;
    } catch (error) {
      connection.close(error);
      child.kill();
      throw EchoAgentError.fromUnknown(error);
    }
  }

  async request<T = unknown>(method: string, params?: unknown): Promise<T> {
    try {
      return await (this.connection.agent as unknown as { request<Response>(method: string, params?: unknown): Promise<Response> })
        .request<T>(method, params);
    } catch (error) {
      throw EchoAgentError.fromUnknown(error, method);
    }
  }

  async notify(method: string, params?: unknown): Promise<void> {
    try {
      await (this.connection.agent as unknown as { notify(method: string, params?: unknown): Promise<void> })
        .notify(method, params);
    } catch (error) {
      throw EchoAgentError.fromUnknown(error, method);
    }
  }

  async registerExtension(
    kind: string,
    implementationId: string,
    descriptor: unknown,
    handler: ExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    const response = await this.request<{ extension: WireHandle }>("_echo_agent/extension/register", {
      kind,
      implementation_id: implementationId,
      descriptor,
      timeout,
    });
    this.extensionHandlers.set(response.extension.id, handler);
    return new ExtensionRegistration(this, response.extension);
  }

  async registerTypedExtension(
    implementationId: string,
    descriptor: ExtensionDescriptor,
    handler: ExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    if (descriptor.descriptor_version !== 1) {
      throw new EchoAgentError("invalid_value", "only extension descriptor_version 1 is supported");
    }
    return this.registerExtension(descriptor.kind, implementationId, descriptor, handler, timeout);
  }

  private async registerExtensionKind(
    expected: ExtensionKind,
    implementationId: string,
    descriptor: ExtensionDescriptor,
    handler: ExtensionHandler,
  ): Promise<ExtensionRegistration> {
    if (descriptor.kind !== expected) {
      throw new EchoAgentError("invalid_value", `expected ${expected} extension descriptor, received ${descriptor.kind}`);
    }
    return this.registerTypedExtension(implementationId, descriptor, handler);
  }

  async registerTool(
    implementationId: string,
    descriptor: ToolDescriptor,
    handler: ToolExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    return this.registerExtension(
      "tool",
      implementationId,
      descriptor,
      typedExtensionHandler("tool", handler),
      timeout,
    );
  }

  async registerLlmClient(
    implementationId: string,
    descriptor: LlmClientDescriptor,
    handler: LlmClientExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    return this.registerExtension(
      "llm_client",
      implementationId,
      descriptor,
      typedExtensionHandler("llm_client", handler),
      timeout,
    );
  }

  async registerStore(
    implementationId: string,
    descriptor: StoreDescriptor,
    handler: StoreExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    return this.registerExtension(
      "store",
      implementationId,
      descriptor,
      typedExtensionHandler("store", handler),
      timeout,
    );
  }

  async registerCritic(
    implementationId: string,
    descriptor: CriticDescriptor,
    handler: CriticExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    return this.registerExtension(
      "critic",
      implementationId,
      descriptor,
      typedExtensionHandler("critic", handler),
      timeout,
    );
  }

  async registerContextCompressor(
    implementationId: string,
    descriptor: ContextCompressorDescriptor,
    handler: ContextCompressorExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    return this.registerExtension(
      "context_compressor",
      implementationId,
      descriptor,
      typedExtensionHandler("context_compressor", handler),
      timeout,
    );
  }

  async registerAgentComponent(
    implementationId: string,
    descriptor: AgentComponentDescriptor,
    handler: AgentComponentExtensionHandler,
    timeout?: { readonly seconds: string; readonly nanos: number },
  ): Promise<ExtensionRegistration> {
    return this.registerExtension(
      "agent_component",
      implementationId,
      descriptor,
      typedExtensionHandler("agent_component", handler),
      timeout,
    );
  }

  async registerHumanLoopProvider(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "human_loop_provider" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("human_loop_provider", implementationId, descriptor, handler);
  }

  async registerHook(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "hook" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("hook", implementationId, descriptor, handler);
  }

  async registerAgentCallback(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "agent_callback" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("agent_callback", implementationId, descriptor, handler);
  }

  async registerInterventionCallback(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "intervention_callback" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("intervention_callback", implementationId, descriptor, handler);
  }

  async registerAgentFactory(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "agent_factory" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("agent_factory", implementationId, descriptor, handler);
  }

  async registerCustomAgent(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "custom_agent" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("custom_agent", implementationId, descriptor, handler);
  }

  async registerChannelPlugin(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "channel_plugin" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("channel_plugin", implementationId, descriptor, handler);
  }

  async registerChannelMessageHandler(implementationId: string, descriptor: ExtensionDescriptor & { readonly kind: "channel_message_handler" }, handler: ExtensionHandler): Promise<ExtensionRegistration> {
    return this.registerExtensionKind("channel_message_handler", implementationId, descriptor, handler);
  }

  async unregisterExtension(extension: WireHandle): Promise<unknown> {
    this.extensionHandlers.delete(extension.id);
    return this.request("_echo_agent/extension/unregister", { extension });
  }

  streamWriter(stream: WireHandle): ExtensionStreamWriter {
    return new ExtensionStreamWriter(this, stream);
  }

  async invoke<T = unknown>(operation: string, handle: WireHandle | undefined, args: readonly unknown[] = []): Promise<T> {
    const value = await this.request<{ value: WireValue }>("_echo_agent/facade/invoke", {
      operation,
      signature_digest: this.catalog.signature(operation),
      handle,
      arguments: args.map(toWireValue),
    });
    return fromWireValue(value.value) as T;
  }

  async countTokens(tokenizer: TokenizerReference, text: string): Promise<bigint> {
    const count = await this.invoke<string>(
      "echo_core::tokenizer::Tokenizer::count_tokens",
      tokenizer.resource,
      [tokenizer.owner_session_id, text],
    );
    return BigInt(count);
  }

  async classifyTurnOutcome(eventWire: WireValue): Promise<WireValue | null> {
    const operation = "echo_orchestration::runtime::turn_driver::TurnOutcome::classify";
    return this.invoke<TurnOutcomeClassification>(operation, undefined, [eventWire]);
  }

  async call<T = unknown>(operation: string, handle: WireHandle | undefined, args: readonly unknown[] = []): Promise<T> {
    const resolved = this.catalog.resolve(operation);
    if (resolved.method === "_echo_agent/facade/invoke") return this.invoke<T>(operation, handle, args);
    return this.family<T>(resolved.family, operation, handle, args);
  }

  async family<T = unknown>(family: string, operation: string, session: WireHandle | undefined, args: readonly unknown[] = []): Promise<T> {
    const method = `_echo_agent/${family}/op`;
    const value = await this.request<{ value: WireValue }>(method, {
      operation,
      signature_digest: this.catalog.familySignature(method, operation),
      handle: session,
      arguments: args.map(toWireValue),
    });
    return fromWireValue(value.value) as T;
  }

  async createAgent(config: unknown = { variant: "host_default" }, idempotencyId?: string): Promise<AgentHandle> {
    const response = await this.request<{ agent: WireHandle }>("_echo_agent/agent/create", {
      config,
      idempotency_id: idempotencyId,
    });
    return new AgentHandle(this, response.agent);
  }

  updatesFor(sessionId: string): AsyncIterable<unknown> {
    let queue = this.updates.get(sessionId);
      if (!queue) {
      queue = new AsyncQueue<unknown>(this.queueCapacity);
      this.updates.set(sessionId, queue);
    }
    return queue;
  }

  eventsFor(streamId: string, handle?: WireHandle): AsyncIterable<FacadeStreamItem> {
    let feed = this.events.get(streamId);
    if (!feed) {
      feed = { queue: new AsyncQueue<FacadeStreamItem>(this.queueCapacity), handle, lastEventSequence: 0n };
      this.events.set(streamId, feed);
    } else if (handle && feed.handle && !sameHandle(feed.handle, handle)) {
      feed.queue.fail(new EchoAgentError("handle_mismatch", "event stream handle changed generation"));
    } else {
      feed.handle ??= handle;
    }
    return this.iterateEvents(feed);
  }

  publishFirstEvent(stream: WireHandle, envelope: WireEventEnvelope): void {
    const event = decodeEvent({ stream, envelope });
    let feed = this.events.get(stream.id);
    if (!feed) {
      feed = { queue: new AsyncQueue<FacadeStreamItem>(this.queueCapacity), handle: stream, lastEventSequence: 0n };
      this.events.set(stream.id, feed);
    }
    if (feed.handle && !sameHandle(feed.handle, stream)) {
      feed.queue.fail(new EchoAgentError("handle_mismatch", "event stream handle changed generation"));
      return;
    }
    const sequence = parsePositiveSequence(event.envelope.sequence);
    if (sequence <= feed.lastEventSequence) return;
    feed.handle = stream;
    feed.lastEventSequence = sequence;
    feed.queue.push(event);
  }

  endEvents(streamId: string): void {
    this.events.get(streamId)?.queue.end();
  }

  endUpdates(sessionId: string): void {
    this.updates.get(sessionId)?.end();
  }

  async acknowledge(stream: WireHandle, sequence: string): Promise<void> {
    const parsed = parsePositiveSequence(sequence);
    await this.notify("_echo_agent/event/ack", {
      ack: { stream, last_processed_sequence: parsed.toString() },
    });
  }

  private async *iterateEvents(feed: StreamFeed): AsyncIterable<FacadeStreamItem> {
    let pendingSequence: string | undefined;
    try {
      for await (const item of feed.queue) {
        if (pendingSequence && feed.handle) {
          await this.acknowledge(feed.handle, pendingSequence);
          pendingSequence = undefined;
        }
        pendingSequence = "gap" in item
          ? item.gap.snapshot_watermark
          : item.envelope.sequence;
        yield item;
      }
    } finally {
      if (pendingSequence && feed.handle && !this.closed) {
        try {
          await this.acknowledge(feed.handle, pendingSequence);
        } catch {
          // The Host may already have exited while the consumer was closing.
        }
      }
    }
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    for (const feed of this.events.values()) feed.queue.end();
    for (const queue of this.updates.values()) queue.end();
    this.connection.close();
    const exited = new Promise<void>((resolve) => {
      if (this.process.exitCode !== null) resolve();
      else this.process.once("exit", () => resolve());
    });
    const settled = await Promise.race([
      exited.then(() => true),
      new Promise<boolean>((resolve) => setTimeout(() => resolve(false), DEFAULT_CLOSE_WAIT_MS)),
    ]);
    if (!settled && !this.process.killed) {
      this.process.kill();
      await exited;
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }
}

export class AgentHandle {
  private closed = false;
  constructor(readonly client: EchoAgentClient, readonly wire: WireHandle) {}
  async describe(): Promise<unknown> {
    if (this.closed) throw new EchoAgentError("closed_handle", "agent handle is closed");
    return this.client.request("_echo_agent/agent/describe", { agent: this.wire });
  }
  async createSession(cwd?: string): Promise<SessionHandle> {
    if (this.closed) throw new EchoAgentError("closed_handle", "agent handle is closed");
    const response = await this.client.request<{ session: WireHandle; acp_session_id: string; task_run?: WireHandle }>(
      "_echo_agent/session/create",
      {
        agent: this.wire,
        working_dir: cwd ? { encoding: "utf8", path: cwd } : undefined,
      },
    );
    return new SessionHandle(this.client, this.wire, response.session, response.acp_session_id, response.task_run);
  }
  async close(): Promise<void> {
    if (this.closed) return;
    await this.client.request("_echo_agent/agent/close", { agent: this.wire });
    this.closed = true;
  }
}

export class ExtensionRegistration {
  private closed = false;
  constructor(readonly client: EchoAgentClient, readonly wire: WireHandle) {}
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;
    await this.client.unregisterExtension(this.wire);
  }
}

export class ExtensionStreamWriter {
  private sequence = 0n;
  private closed = false;
  constructor(readonly client: EchoAgentClient, readonly stream: WireHandle) {}
  private next(): string {
    if (this.closed) throw new Error("extension stream is already closed");
    if (this.sequence === (1n << 64n) - 1n) throw new RangeError("extension stream sequence exceeds u64");
    this.sequence += 1n;
    return this.sequence.toString();
  }
  async chunk(value: unknown): Promise<void> {
    await this.client.notify("_echo_agent/extension/stream", {
      event: "chunk", stream: this.stream, sequence: this.next(), value,
    });
  }
  async agentComponentChunk(value: AgentComponentStreamChunk): Promise<void> {
    await this.chunk({ kind: "agent_component", value });
  }
  async complete(value: unknown): Promise<void> {
    await this.client.notify("_echo_agent/extension/stream", {
      event: "complete", stream: this.stream, sequence: this.next(), value,
    });
    this.closed = true;
  }
  async agentComponentComplete(value: AgentComponentStreamComplete): Promise<void> {
    await this.complete({ kind: "agent_component", value });
  }
  async failed(error: unknown): Promise<void> {
    await this.client.notify("_echo_agent/extension/stream", {
      event: "failed", stream: this.stream, sequence: this.next(), error,
    });
    this.closed = true;
  }
  async cancelled(): Promise<void> {
    await this.client.notify("_echo_agent/extension/stream", {
      event: "cancelled", stream: this.stream, sequence: this.next(),
    });
    this.closed = true;
  }
}

export class SessionHandle {
  private closed = false;
  constructor(
    readonly client: EchoAgentClient,
    readonly agent: WireHandle,
    readonly wire: WireHandle,
    readonly acpSessionId: string,
    readonly taskRun?: WireHandle,
  ) {}

  async prompt(text: string, options?: { readonly signal?: AbortSignal }): Promise<unknown> {
    this.ensureOpen();
    const request = this.client.request("session/prompt", {
      sessionId: this.acpSessionId,
      prompt: [{ type: "text", text }],
    });
    return this.abortable(request, options?.signal, () => this.cancel());
  }

  updates(): AsyncIterable<unknown> {
    this.ensureOpen();
    return this.client.updatesFor(this.acpSessionId);
  }

  async startRun(input: { readonly mode?: "chat" | "execute"; readonly text: string }): Promise<RunHandle> {
    this.ensureOpen();
    const response = await this.client.request<RunStartResult>("_echo_agent/run/start", {
      session: this.wire,
      input: { kind: input.mode === "execute" ? "execute" : "chat", text: input.text },
    });
    if (response.first_event) this.client.publishFirstEvent(response.stream, response.first_event);
    return new RunHandle(this.client, response.run, response.stream);
  }

  async invoke<T = unknown>(operation: string, args: readonly unknown[] = []): Promise<T> {
    this.ensureOpen();
    const resolved = this.client.catalog.resolve(operation);
    return resolved.method === "_echo_agent/facade/invoke"
      ? this.client.call<T>(operation, this.agent, [this.wire, ...args])
      : this.client.call<T>(operation, this.wire, args);
  }

  async cancel(): Promise<void> {
    this.ensureOpen();
    await this.client.notify("session/cancel", { sessionId: this.acpSessionId });
  }

  async close(): Promise<void> {
    if (this.closed) return;
    await this.client.request("_echo_agent/session/close", { session: this.wire });
    this.closed = true;
    this.client.endUpdates(this.acpSessionId);
  }

  private ensureOpen(): void {
    if (this.closed) throw new EchoAgentError("closed_handle", "session handle is closed");
  }

  private async abortable<T>(request: Promise<T>, signal: AbortSignal | undefined, cancel: () => Promise<void>): Promise<T> {
    if (!signal) return request;
    if (signal.aborted) {
      await cancel().catch(() => undefined);
      throw new EchoAgentError("cancelled", "session operation was cancelled");
    }
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const onAbort = () => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        void cancel().catch(() => undefined);
        reject(new EchoAgentError("cancelled", "session operation was cancelled"));
      };
      signal.addEventListener("abort", onAbort, { once: true });
      request.then((value) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        resolve(value);
      }, (error) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        reject(error);
      });
    });
  }
}

export class RunHandle {
  private closed = false;
  constructor(readonly client: EchoAgentClient, readonly wire: WireHandle, readonly stream: WireHandle) {}
  get events(): AsyncIterable<FacadeStreamItem> { return this.client.eventsFor(this.stream.id, this.stream); }
  get(): Promise<RunGetResult> {
    this.ensureOpen();
    return this.client.request<RunGetResult>("_echo_agent/run/get", { run: this.wire });
  }
  status(): Promise<TurnStatus> {
    this.ensureOpen();
    return this.client.call<TurnStatus>(
      "echo_orchestration::runtime::turn_driver::TurnReceipt::status",
      this.wire,
      [],
    );
  }
  outcomeStatus(): Promise<TurnStatus> {
    this.ensureOpen();
    return this.client.call<TurnStatus>(
      "echo_orchestration::runtime::turn_driver::TurnOutcome::status",
      this.wire,
      [],
    );
  }
  usage(): Promise<ExecutionUsage> {
    this.ensureOpen();
    return this.client.call<ExecutionUsage>(
      "echo_orchestration::runtime::turn_driver::TurnReceipt::usage",
      this.wire,
      [],
    );
  }
  wait(options?: { readonly timeoutMs?: number; readonly signal?: AbortSignal }): Promise<RunWaitResult> {
    this.ensureOpen();
    const timeout = options?.timeoutMs === undefined ? undefined : durationForMilliseconds(options.timeoutMs);
    const request = this.client.request<RunWaitResult>("_echo_agent/run/wait", { run: this.wire, timeout });
    return this.abortable(request, options?.signal, () => this.cancel().then(() => undefined));
  }
  cancel(): Promise<RunCancelResult> {
    this.ensureOpen();
    return this.client.request<RunCancelResult>("_echo_agent/run/cancel", { run: this.wire });
  }
  steer(text: string): Promise<unknown> {
    this.ensureOpen();
    return this.client.request("_echo_agent/run/steer", { run: this.wire, text });
  }
  async replay(afterSequence = "0", limit = 512): Promise<ReplayResult> {
    this.ensureOpen();
    const result = await this.client.request<{
      requested_after_sequence: string;
      events: readonly WireEventEnvelope[];
      next_cursor: { stream_id: string; last_processed_sequence: string };
      gap?: { stream: WireHandle; gap: EventGap };
    }>("_echo_agent/run/replay", {
      stream: this.stream,
      after_sequence: afterSequence,
      max_events: String(limit),
    });
    if (result.next_cursor.stream_id !== this.stream.id) {
      throw new EchoAgentError("serialization_violation", "replay cursor stream does not match Run stream");
    }
    return {
      requested_after_sequence: result.requested_after_sequence,
      events: result.events.map((envelope) => decodeEvent({ stream: this.stream, envelope })),
      next_cursor: result.next_cursor,
      gap: result.gap ? decodeGap(result.gap) : undefined,
    };
  }
  async close(): Promise<void> {
    if (this.closed) return;
    const state = await this.get().catch((error) => { throw error; });
    if (!isTerminalStatus(state.status)) {
      await this.cancel();
      const settled = await this.wait({ timeoutMs: DEFAULT_CLOSE_WAIT_MS });
      if (!settled.settled) throw new EchoAgentError("timeout", "run did not settle before close timeout", "after_delay");
    }
    this.closed = true;
    this.client.endEvents(this.stream.id);
  }

  private ensureOpen(): void {
    if (this.closed) throw new EchoAgentError("closed_handle", "run handle is closed");
  }

  private async abortable<T>(request: Promise<T>, signal: AbortSignal | undefined, cancel: () => Promise<void>): Promise<T> {
    if (!signal) return request;
    if (signal.aborted) {
      await cancel().catch(() => undefined);
      throw new EchoAgentError("cancelled", "run operation was cancelled");
    }
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const onAbort = () => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        void cancel().catch(() => undefined);
        reject(new EchoAgentError("cancelled", "run operation was cancelled"));
      };
      signal.addEventListener("abort", onAbort, { once: true });
      request.then((value) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        resolve(value);
      }, (error) => {
        if (settled) return;
        settled = true;
        signal.removeEventListener("abort", onAbort);
        reject(error);
      });
    });
  }
}

function readNonEmptyString(value: unknown, key: string): string | undefined {
  if (typeof value !== "object" || value === null || !(key in value)) return undefined;
  const candidate = (value as Record<string, unknown>)[key];
  return typeof candidate === "string" && candidate.trim().length > 0 ? candidate : undefined;
}

function parsePositiveSequence(value: unknown): bigint {
  if (typeof value !== "string" || !/^\d+$/u.test(value)) {
    throw new EchoAgentError("serialization_violation", "event sequence must be decimal text");
  }
  try {
    const sequence = BigInt(value);
    if (sequence < 1n || sequence > ((1n << 64n) - 1n) || sequence.toString() !== value) {
      throw new EchoAgentError("serialization_violation", "event sequence is outside the u64 range");
    }
    return sequence;
  } catch (error) {
    if (error instanceof EchoAgentError) throw error;
    throw new EchoAgentError("serialization_violation", "event sequence is not a valid u64");
  }
}

function isCanonicalU64Text(value: unknown): value is string {
  if (typeof value !== "string" || !/^(?:0|[1-9]\d*)$/u.test(value)) return false;
  try {
    const parsed = BigInt(value);
    return parsed <= ((1n << 64n) - 1n) && parsed.toString() === value;
  } catch {
    return false;
  }
}

function sameHandle(left: WireHandle, right: WireHandle): boolean {
  return left.id === right.id && left.generation === right.generation && left.kind === right.kind;
}

function decodeEvent(value: unknown): FacadeEvent {
  if (typeof value !== "object" || value === null) {
    throw new EchoAgentError("serialization_violation", "event notification must be an object");
  }
  const candidate = value as Record<string, unknown>;
  const stream = candidate.stream;
  if (!isWireHandle(stream) || stream.kind !== "stream") {
    throw new EchoAgentError("serialization_violation", "event notification has an invalid stream handle");
  }
  const envelope = candidate.envelope;
  if (typeof envelope !== "object" || envelope === null) {
    throw new EchoAgentError("serialization_violation", "event notification is missing its envelope");
  }
  const entry = envelope as Record<string, unknown>;
  const eventId = readNonEmptyString(entry, "event_id");
  const contentHash = readNonEmptyString(entry, "content_hash");
  const streamId = readNonEmptyString(entry, "stream_id");
  const turnId = readNonEmptyString(entry, "turn_id");
  if (!eventId || !contentHash || !streamId || !turnId || !/^sha256:[0-9a-f]{64}$/iu.test(contentHash)) {
    throw new EchoAgentError("serialization_violation", "event envelope identity is malformed");
  }
  if (streamId !== stream.id) {
    throw new EchoAgentError("serialization_violation", "event envelope stream_id does not match its handle");
  }
  const sequence = readNonEmptyString(entry, "sequence");
  if (!sequence) throw new EchoAgentError("serialization_violation", "event envelope is missing sequence");
  parsePositiveSequence(sequence);
  const payload = entry.payload;
  if (typeof payload !== "object" || payload === null) {
    throw new EchoAgentError("serialization_violation", "event envelope payload is missing");
  }
  const payloadRecord = payload as Record<string, unknown>;
  const eventType = readNonEmptyString(payloadRecord, "event_type");
  if (!eventType) throw new EchoAgentError("serialization_violation", "event envelope payload is malformed");
  const schemaVersion = entry.schema_version;
  if (typeof schemaVersion !== "number" || !Number.isInteger(schemaVersion) || schemaVersion < 1) {
    throw new EchoAgentError("serialization_violation", "event envelope schema_version is malformed");
  }
  const timestamp = entry.timestamp;
  if (typeof timestamp !== "object" || timestamp === null) {
    throw new EchoAgentError("serialization_violation", "event envelope timestamp is malformed");
  }
  const timestampRecord = timestamp as Record<string, unknown>;
  const unixSeconds = readNonEmptyString(timestampRecord, "unix_seconds");
  const nanos = timestampRecord.nanos;
  if (!unixSeconds || typeof nanos !== "number" || !Number.isInteger(nanos) || nanos < 0 || nanos >= 1_000_000_000) {
    throw new EchoAgentError("serialization_violation", "event envelope timestamp is malformed");
  }
  try {
    const parsed = BigInt(unixSeconds);
    if (parsed < -(1n << 63n) || parsed > (1n << 63n) - 1n || parsed.toString() !== unixSeconds) {
      throw new Error("timestamp range");
    }
  } catch {
    throw new EchoAgentError("serialization_violation", "event envelope timestamp seconds are malformed");
  }
  return {
    stream,
    envelope: {
      schema_version: schemaVersion,
      event_id: eventId,
      content_hash: contentHash,
      sequence,
      stream_id: streamId,
      conversation_id: readNonEmptyString(entry, "conversation_id"),
      run_id: readNonEmptyString(entry, "run_id"),
      turn_id: turnId,
      message_id: readNonEmptyString(entry, "message_id"),
      execution_id: readNonEmptyString(entry, "execution_id"),
      parent_event_id: readNonEmptyString(entry, "parent_event_id"),
      timestamp: {
        unix_seconds: unixSeconds,
        nanos,
        ...(readNonEmptyString(timestampRecord, "rfc3339") === undefined
          ? {}
          : { rfc3339: readNonEmptyString(timestampRecord, "rfc3339") }),
      },
      payload: {
        event_type: eventType,
        ...(payloadRecord.data === undefined ? {} : { data: payloadRecord.data as WireValue }),
      },
    },
  };
}

function decodeGap(value: unknown): FacadeGap {
  if (typeof value !== "object" || value === null) {
    throw new EchoAgentError("serialization_violation", "gap notification must be an object");
  }
  const candidate = value as Record<string, unknown>;
  const stream = candidate.stream;
  if (!isWireHandle(stream) || stream.kind !== "stream") {
    throw new EchoAgentError("serialization_violation", "gap notification has an invalid stream handle");
  }
  const gap = candidate.gap;
  if (typeof gap !== "object" || gap === null) {
    throw new EchoAgentError("serialization_violation", "gap notification is missing its gap");
  }
  const entry = gap as Record<string, unknown>;
  const from = readNonEmptyString(entry, "from_sequence");
  const to = readNonEmptyString(entry, "to_sequence");
  const watermark = readNonEmptyString(entry, "snapshot_watermark");
  const reason = readNonEmptyString(entry, "reason");
  if (!from || !to || !watermark || !reason) {
    throw new EchoAgentError("serialization_violation", "gap fields are malformed");
  }
  const fromValue = parsePositiveSequence(from);
  const toValue = parsePositiveSequence(to);
  const watermarkValue = parsePositiveSequence(watermark);
  if (toValue < fromValue || watermarkValue < toValue) {
    throw new EchoAgentError("serialization_violation", "gap sequence range is malformed");
  }
  return { stream, gap: { from_sequence: from, to_sequence: to, reason, snapshot_watermark: watermark } };
}

function failFeeds(feeds: Map<string, StreamFeed>, error: EchoAgentError): void {
  for (const feed of feeds.values()) feed.queue.fail(error);
}

function failQueues(queues: Map<string, AsyncQueue<unknown>>, error: EchoAgentError): void {
  for (const queue of queues.values()) queue.fail(error);
}

function durationForMilliseconds(value: number): { seconds: string; nanos: number } {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new RangeError("timeoutMs must be a non-negative safe integer");
  }
  const seconds = Math.floor(value / 1000);
  const nanos = (value % 1000) * 1_000_000;
  return { seconds: String(seconds), nanos };
}

function isTerminalStatus(status: string): boolean {
  return status === "completed" || status === "cancelled" || status === "failed" || status === "interrupted" || status === "closed";
}

function validateCapability(capability: EchoAgentCapability, options: SpawnOptions): void {
  for (const feature of options.requiredFeatures ?? []) {
    if (!capability.features.includes(feature)) throw new EchoAgentError("feature_unavailable", `Host lacks feature ${feature}`);
  }
  for (const required of options.requiredCapabilities ?? []) {
    if (!capability.capabilities.some((entry) => entry.capability === required)) {
      throw new EchoAgentError("extension_capability_mismatch", `Host lacks capability ${required}`);
    }
  }
  if (options.contractDigest && capability.contract_digest !== options.contractDigest) {
    throw new EchoAgentError("extension_capability_mismatch", "contract digest mismatch");
  }
  if (options.sourceContractDigest && capability.source_contract_digest !== options.sourceContractDigest) {
    throw new EchoAgentError("extension_capability_mismatch", "source contract digest mismatch");
  }
}

function cancelledOutcome(): ExtensionInvokeOutcome {
  return {
    outcome: "error",
    error: { code: "cancelled", message: "extension invocation was cancelled", retryable: "never" },
  };
}

function normalizeOutcome(value: unknown): ExtensionInvokeOutcome {
  if (typeof value !== "object" || value === null || !("outcome" in value)) {
    return {
      outcome: "error",
      error: { code: "invalid_value", message: "extension handler must return an outcome", retryable: "never" },
    };
  }
  const candidate = value as Record<string, unknown>;
  if (candidate.outcome === "result" && "result" in candidate) {
    return candidate as ExtensionInvokeOutcome;
  }
  if (candidate.outcome === "stream" && isWireHandle(candidate.stream)) {
    return candidate as ExtensionInvokeOutcome;
  }
  if (candidate.outcome === "error" && typeof candidate.error === "object" && candidate.error !== null) {
    return candidate as ExtensionInvokeOutcome;
  }
  return {
    outcome: "error",
    error: { code: "invalid_value", message: "extension handler returned a malformed outcome", retryable: "never" },
  };
}

function componentForOperation(operation: string): string | undefined {
  if (operation.startsWith("conversation_")) return "conversation_store";
  if (operation.startsWith("run_")) return "run_store";
  if (operation.startsWith("runtime_")) return "runtime_state_store";
  if (operation.startsWith("audit_")) return "audit_logger";
  if (operation === "context_project") return "context_projector";
  if (operation === "memory_trigger") return "memory_trigger_sink";
  if (operation === "guard_check") return "guard";
  if (operation === "search_provider_search") return "search_provider";
  if (operation.startsWith("workflow_checkpoint_")) return "workflow_checkpoint_store";
  if (operation.startsWith("revisioned_task_")) return "revisioned_task_store";
  if (operation.startsWith("sandbox_")) return "sandbox_executor";
  if (operation.startsWith("mcp_transport_")) return "mcp_transport";
  if (operation === "embedder_embed") return "embedder";
  if (operation === "memory_promoter_promote") return "memory_promoter";
  if (operation === "workflow_run") return "workflow";
  if (operation === "workflow_run_stream") return "workflow";
  if (operation === "intent_classify") return "intent_classifier";
  if (operation === "skill_load_allows") return "skill_load_policy";
  return undefined;
}

const AGENT_COMPONENT_UNIT_INPUTS = new Set([
  "workflow_checkpoint_list",
  "workflow_checkpoint_clear",
  "sandbox_is_available",
  "sandbox_cleanup",
  "mcp_transport_close",
  "mcp_transport_try_notification",
]);

const AGENT_COMPONENT_INPUT_FIELDS: Readonly<Record<string, readonly string[]>> = {
  conversation_create: ["conversation"],
  conversation_get: ["conversation_id"],
  conversation_list: ["user_id", "agent_type", "limit", "offset"],
  conversation_update: ["conversation_id", "title", "summary", "compressed_before_id"],
  conversation_delete: ["conversation_id"],
  conversation_save_messages: ["conversation_id", "messages"],
  conversation_get_messages: ["conversation_id"],
  conversation_count_messages: ["conversation_id"],
  conversation_ensure: ["conversation"],
  conversation_search: ["query", "limit"],
  run_save: ["run"],
  run_load: ["run_id"],
  run_list_by_session: ["session_id"],
  run_list_all: ["limit"],
  run_append_event: ["run_id", "event"],
  run_list_by_parent: ["parent_run_id"],
  runtime_get_checkpoint: ["conversation_id"],
  runtime_save_checkpoint: ["checkpoint"],
  runtime_save_checkpoint_for_scope: ["scope_id", "checkpoint"],
  runtime_state_ids: ["scope_id"],
  runtime_clear_state: ["scope_id", "runtime_state_id"],
  runtime_clear_scope: ["scope_id"],
  runtime_clear_conversation: ["conversation_id"],
  audit_log: ["event"],
  audit_query: ["session_id", "agent_name", "from", "to", "limit"],
  context_project: ["iteration", "agent_name", "session_id", "conversation_id", "run_id", "turn_id"],
  memory_trigger: ["trigger"],
  guard_check: ["content", "direction"],
  search_provider_search: ["query", "max_results"],
  workflow_checkpoint_save: ["checkpoint"],
  workflow_checkpoint_load: ["checkpoint_id"],
  workflow_checkpoint_claim: ["checkpoint_id"],
  workflow_checkpoint_list: [],
  workflow_checkpoint_list_by_graph: ["graph_name"],
  workflow_checkpoint_list_filtered: ["filter"],
  workflow_checkpoint_delete: ["checkpoint_id"],
  workflow_checkpoint_clear: [],
  revisioned_task_load: ["scope_id"],
  revisioned_task_compare_and_commit: ["scope_id", "commit"],
  sandbox_is_available: [],
  sandbox_execute: ["command"],
  sandbox_execute_stream: ["command"],
  sandbox_execute_with_limits: ["command", "limits"],
  sandbox_execute_with_limits_and_cancel: ["command", "limits"],
  sandbox_cleanup: [],
  mcp_transport_send: ["request"],
  mcp_transport_notify: ["notification"],
  mcp_transport_close: [],
  mcp_transport_try_notification: [],
  embedder_embed: ["text"],
  memory_promoter_promote: ["evicted"],
  workflow_run: ["input"],
  workflow_run_stream: ["input"],
  intent_classify: ["user_input", "context"],
  skill_load_allows: ["descriptor"],
};

function hasNoUnknownFields(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const accepted = new Set(allowed);
  return Object.keys(value).every((field) => accepted.has(field));
}

function optionalText(value: unknown): boolean {
  return value === undefined || value === null || typeof value === "string";
}

function requiredText(input: Record<string, unknown>, field: string): boolean {
  return typeof input[field] === "string";
}

function optionalU64(value: unknown): boolean {
  return value === undefined || value === null || isCanonicalU64Text(value);
}

function validateAgentComponentCall(component: string, call: Record<string, unknown>): void {
  const operation = typeof call.operation === "string" ? call.operation : "";
  const input = call.input === undefined || call.input === null
    ? AGENT_COMPONENT_UNIT_INPUTS.has(operation) ? {} : call.input
    : call.input;
  if (componentForOperation(operation) !== component || !isRecord(input)) {
    throw new EchoAgentError("serialization_violation", "Agent component call kind/operation is invalid");
  }
  const inputFields = AGENT_COMPONENT_INPUT_FIELDS[operation];
  if (inputFields === undefined || !hasNoUnknownFields(input, inputFields)) {
    throw new EchoAgentError("serialization_violation", `Agent component ${operation} has unknown input fields`);
  }
  const wire = (field: string) => isWireValue(input[field]);
  const text = (field: string) => requiredText(input, field);
  const valid = (() => {
    switch (operation) {
      case "conversation_create": return wire("conversation");
      case "conversation_get": case "conversation_delete":
      case "conversation_get_messages": case "conversation_count_messages":
      case "runtime_get_checkpoint": case "runtime_clear_conversation":
        return text("conversation_id");
      case "conversation_list":
        return optionalText(input.user_id) && optionalText(input.agent_type)
          && optionalU64(input.limit) && optionalU64(input.offset);
      case "conversation_update":
        return text("conversation_id") && optionalText(input.title) && optionalText(input.summary)
          && (input.compressed_before_id === undefined || input.compressed_before_id === null
            || (typeof input.compressed_before_id === "string" && /^-?(0|[1-9]\d*)$/u.test(input.compressed_before_id)));
      case "conversation_save_messages":
        return text("conversation_id") && Array.isArray(input.messages) && input.messages.every(isWireValue);
      case "conversation_ensure": return wire("conversation");
      case "conversation_search": return text("query") && isCanonicalU64Text(input.limit);
      case "run_save": return wire("run");
      case "run_load": return text("run_id");
      case "run_list_by_session": return text("session_id");
      case "run_list_all": return isCanonicalU64Text(input.limit);
      case "run_append_event": return text("run_id") && wire("event");
      case "run_list_by_parent": return text("parent_run_id");
      case "runtime_save_checkpoint": return wire("checkpoint");
      case "runtime_save_checkpoint_for_scope": return text("scope_id") && wire("checkpoint");
      case "runtime_state_ids": case "runtime_clear_scope": return text("scope_id");
      case "runtime_clear_state": return text("scope_id") && text("runtime_state_id");
      case "audit_log": return wire("event");
      case "audit_query":
        return optionalText(input.session_id) && optionalText(input.agent_name)
          && optionalText(input.from) && optionalText(input.to) && optionalU64(input.limit);
      case "context_project":
        return isCanonicalU64Text(input.iteration) && text("agent_name")
          && optionalText(input.session_id) && optionalText(input.conversation_id)
          && optionalText(input.run_id) && optionalText(input.turn_id);
      case "memory_trigger": return wire("trigger");
      case "guard_check":
        return text("content") && ["input", "output", "tool_input", "tool_output"].includes(String(input.direction));
      case "search_provider_search": return text("query") && isCanonicalU64Text(input.max_results);
      case "workflow_checkpoint_save": return wire("checkpoint");
      case "workflow_checkpoint_load": case "workflow_checkpoint_claim": case "workflow_checkpoint_delete":
        return text("checkpoint_id");
      case "workflow_checkpoint_list": case "workflow_checkpoint_clear":
      case "sandbox_is_available": case "sandbox_cleanup": case "mcp_transport_close":
      case "mcp_transport_try_notification": return Object.keys(input).length === 0;
      case "workflow_checkpoint_list_by_graph": return text("graph_name");
      case "workflow_checkpoint_list_filtered": return wire("filter");
      case "revisioned_task_load": return text("scope_id");
      case "revisioned_task_compare_and_commit": return text("scope_id") && wire("commit");
      case "sandbox_execute": case "sandbox_execute_stream": return wire("command");
      case "sandbox_execute_with_limits": case "sandbox_execute_with_limits_and_cancel":
        return wire("command") && wire("limits");
      case "mcp_transport_send": return wire("request");
      case "mcp_transport_notify": return wire("notification");
      case "embedder_embed": return text("text");
      case "memory_promoter_promote":
        return Array.isArray(input.evicted) && input.evicted.every(isRecord);
      case "workflow_run": return text("input");
      case "workflow_run_stream": return text("input");
      case "intent_classify":
        return text("user_input") && Array.isArray(input.context) && input.context.every(isRecord);
      case "skill_load_allows": return isRecord(input.descriptor);
      default: return false;
    }
  })();
  if (!valid) {
    throw new EchoAgentError(
      "serialization_violation",
      `Agent component ${operation} input does not match its typed contract`,
    );
  }
}

function validateAgentComponentResult(component: string, expected: string, result: unknown): void {
  if (!isRecord(result) || result.operation !== expected || componentForOperation(expected) !== component) {
    throw new EchoAgentError("invalid_value", "Agent component result operation does not match the callback");
  }
  const value = result.value;
  const unit = new Set([
    "conversation_update", "conversation_delete", "conversation_save_messages", "run_save",
    "run_append_event",
    "runtime_save_checkpoint", "runtime_save_checkpoint_for_scope", "runtime_clear_conversation", "audit_log",
    "workflow_checkpoint_save", "workflow_checkpoint_delete", "workflow_checkpoint_clear",
    "sandbox_cleanup", "mcp_transport_notify", "mcp_transport_close",
  ]);
  if (unit.has(expected)) {
    if (value !== undefined) throw new EchoAgentError("invalid_value", `${expected} must return a unit result`);
    return;
  }
  if (!isRecord(value)) throw new EchoAgentError("invalid_value", `${expected} must return a typed value object`);
  const resultFields: Readonly<Record<string, readonly string[]>> = {
    conversation_create: ["conversation"], conversation_get: ["conversation"],
    conversation_list: ["conversations"], conversation_get_messages: ["messages"],
    conversation_count_messages: ["count"], run_load: ["run"],
    conversation_ensure: ["conversation"], conversation_search: ["conversations"],
    run_list_by_session: ["runs"], run_list_all: ["runs"],
    run_list_by_parent: ["runs"],
    runtime_get_checkpoint: ["checkpoint"], runtime_state_ids: ["state_ids"],
    runtime_clear_state: ["receipt"], runtime_clear_scope: ["receipt"],
    audit_query: ["events"], context_project: ["projections"], memory_trigger: ["disposition"],
    guard_check: ["result"], search_provider_search: ["results"],
    workflow_checkpoint_load: ["checkpoint"], workflow_checkpoint_claim: ["checkpoint"],
    workflow_checkpoint_list: ["checkpoints"], workflow_checkpoint_list_by_graph: ["checkpoints"],
    workflow_checkpoint_list_filtered: ["checkpoints"], revisioned_task_load: ["graph"],
    revisioned_task_compare_and_commit: ["graph"], sandbox_is_available: ["available"],
    sandbox_execute: ["result"], sandbox_execute_with_limits: ["result"],
    sandbox_execute_with_limits_and_cancel: ["result"],
    mcp_transport_send: ["response"], mcp_transport_try_notification: ["notification"],
    embedder_embed: ["vector"],
    memory_promoter_promote: ["submitted", "promoted", "deduplicated"],
    workflow_run: ["output"],
    intent_classify: ["intent"],
    skill_load_allows: ["allowed"],
  };
  const expectedFields = resultFields[expected];
  if (expectedFields === undefined || !hasNoUnknownFields(value, expectedFields)) {
    throw new EchoAgentError("invalid_value", `${expected} result has unknown fields`);
  }
  const values = (field: string) => Array.isArray(value[field]) && value[field].every(isWireValue);
  const valid = (() => {
    switch (expected) {
      case "conversation_create": return isWireValue(value.conversation);
      case "conversation_get": return value.conversation === null || value.conversation === undefined || isWireValue(value.conversation);
      case "conversation_list": return values("conversations");
      case "conversation_get_messages": return values("messages");
      case "conversation_count_messages": return isCanonicalU64Text(value.count);
      case "conversation_ensure": return isWireValue(value.conversation);
      case "conversation_search": return values("conversations");
      case "run_load": return value.run === null || value.run === undefined || isWireValue(value.run);
      case "run_list_by_session": case "run_list_all": case "run_list_by_parent": return values("runs");
      case "runtime_get_checkpoint": return value.checkpoint === null || value.checkpoint === undefined || isWireValue(value.checkpoint);
      case "runtime_state_ids": return Array.isArray(value.state_ids) && value.state_ids.every((item) => typeof item === "string");
      case "runtime_clear_state": case "runtime_clear_scope": return isWireValue(value.receipt);
      case "audit_query": return values("events");
      case "context_project": return values("projections");
      case "memory_trigger": return value.disposition === "persist" || value.disposition === "captured";
      case "guard_check": return isWireValue(value.result);
      case "search_provider_search": return values("results");
      case "workflow_checkpoint_load": case "workflow_checkpoint_claim":
        return value.checkpoint === null || value.checkpoint === undefined || isWireValue(value.checkpoint);
      case "workflow_checkpoint_list": case "workflow_checkpoint_list_by_graph": case "workflow_checkpoint_list_filtered":
        return values("checkpoints");
      case "revisioned_task_load": return value.graph === null || value.graph === undefined || isWireValue(value.graph);
      case "revisioned_task_compare_and_commit": return isWireValue(value.graph);
      case "sandbox_is_available": return typeof value.available === "boolean";
      case "sandbox_execute": case "sandbox_execute_with_limits":
      case "sandbox_execute_with_limits_and_cancel": return isWireValue(value.result);
      case "mcp_transport_send": return isWireValue(value.response);
      case "mcp_transport_try_notification":
        return value.notification === null || value.notification === undefined || isWireValue(value.notification);
      case "embedder_embed":
        return Array.isArray(value.vector)
          && value.vector.every((item) => typeof item === "number" && Number.isFinite(item));
      case "memory_promoter_promote":
        return isCanonicalU64Text(value.submitted)
          && isCanonicalU64Text(value.promoted)
          && isCanonicalU64Text(value.deduplicated);
      case "workflow_run": return isWireValue(value.output);
      case "intent_classify": return isWireValue(value.intent);
      case "skill_load_allows": return typeof value.allowed === "boolean";
      default: return false;
    }
  })();
  if (!valid) throw new EchoAgentError("invalid_value", `${expected} result does not match its typed contract`);
}

/**
 * Decode the Host-owned reverse invocation envelope for a typed extension.
 * This is deliberately only a wire/shape adapter; execution and settlement
 * remain in the Rust Host and the official ACP connection.
 */
export function decodeExtensionInvokeCall<K extends "tool" | "llm_client" | "store" | "critic" | "context_compressor" | "agent_component">(
  value: unknown,
  expectedKind: K,
): ExtensionInvokeCall<
  K extends "tool" ? ToolInvocation
    : K extends "llm_client" ? LlmInvocation
      : K extends "store" ? StoreInvocation
        : K extends "critic" ? CriticInvocation
          : K extends "context_compressor" ? ContextCompressorInvocation
            : AgentComponentInvocation
> {
  if (!isRecord(value)) throw new EchoAgentError("serialization_violation", "extension invocation must be an object");
  const extension = value.extension;
  if (!isWireHandle(extension) || extension.kind !== "extension") {
    throw new EchoAgentError("serialization_violation", "extension invocation has an invalid extension handle");
  }
  const invocationId = value.invocation_id;
  if (typeof invocationId !== "string" || invocationId.trim().length === 0) {
    throw new EchoAgentError("serialization_violation", "extension invocation_id must be a non-empty string");
  }
  const deadline = value.deadline;
  if (!isRecord(deadline)
    || typeof deadline.seconds !== "string"
    || !/^\d+$/u.test(deadline.seconds)
    || (deadline.seconds.length > 1 && deadline.seconds.startsWith("0"))
    || typeof deadline.nanos !== "number"
    || !Number.isInteger(deadline.nanos)
    || deadline.nanos < 0
    || deadline.nanos >= 1_000_000_000) {
    throw new EchoAgentError("serialization_violation", "extension invocation deadline is malformed");
  }
  const invocation = value.invocation;
  if (!isRecord(invocation) || typeof invocation.operation !== "string" || !("input" in invocation)) {
    throw new EchoAgentError("serialization_violation", "extension invocation payload is malformed");
  }
  if (extensionKindForOperation(invocation.operation) !== expectedKind) {
    throw new EchoAgentError(
      "invalid_value",
      `${invocation.operation} is not a ${expectedKind} extension operation`,
      "never",
      "_echo_agent/extension/invoke",
    );
  }
  if (expectedKind === "critic") {
    const input = invocation.input;
    if (!isRecord(input)
      || typeof input.task !== "string"
      || typeof input.answer !== "string"
      || typeof input.context !== "string") {
      throw new EchoAgentError(
        "serialization_violation",
        "critique invocation input must contain task, answer, and context strings",
      );
    }
  }
  if (expectedKind === "context_compressor") {
    const input = invocation.input;
    if (!isRecord(input)
      || !Array.isArray(input.messages)
      || !input.messages.every(isRecord)
      || !isCanonicalU64Text(input.token_limit)
      || (input.current_query !== undefined && input.current_query !== null
        && typeof input.current_query !== "string")
      || (input.focus_instructions !== undefined && input.focus_instructions !== null
        && typeof input.focus_instructions !== "string")
      || !isRecord(input.tokenizer)
      || !isWireHandle(input.tokenizer.resource)
      || input.tokenizer.resource.kind !== "facade_resource"
      || typeof input.tokenizer.owner_session_id !== "string"
      || input.tokenizer.owner_session_id.length === 0) {
      throw new EchoAgentError(
        "serialization_violation",
        "compressor invocation requires messages and canonical token_limit",
      );
    }
  }
  if (expectedKind === "agent_component") {
    const input = invocation.input;
    const call = isRecord(input) ? input.call : undefined;
    if (!isRecord(input)
      || typeof input.component !== "string"
      || !isRecord(call)
      || typeof call.operation !== "string"
      || (!("input" in call) && !AGENT_COMPONENT_UNIT_INPUTS.has(call.operation))) {
      throw new EchoAgentError(
        "serialization_violation",
        "agent component invocation requires component and an operation-discriminated call",
      );
    }
    const normalizedCall = !("input" in call) && AGENT_COMPONENT_UNIT_INPUTS.has(call.operation)
      ? { ...call, input: {} }
      : call;
    validateAgentComponentCall(input.component, normalizedCall);
    if (normalizedCall !== call) {
      invocation.input = { ...input, call: normalizedCall };
    }
  }
  if (value.stream !== undefined && value.stream !== null
    && (!isWireHandle(value.stream) || value.stream.kind !== "stream")) {
    throw new EchoAgentError("serialization_violation", "extension stream handle is malformed");
  }
  return {
    context: value.context as ExtensionInvokeCall["context"],
    deadline: { seconds: deadline.seconds, nanos: deadline.nanos },
    extension,
    invocation: invocation as ExtensionInvokeCall["invocation"],
    invocation_id: invocationId,
    stream: value.stream as WireHandle | null | undefined,
  } as ExtensionInvokeCall<
    K extends "tool" ? ToolInvocation
      : K extends "llm_client" ? LlmInvocation
      : K extends "store" ? StoreInvocation
          : K extends "critic" ? CriticInvocation
            : K extends "context_compressor" ? ContextCompressorInvocation
              : AgentComponentInvocation
  >;
}

function typedExtensionHandler<I extends ExtensionInvocation, R extends ExtensionResult>(
  expectedKind: "tool" | "llm_client" | "store" | "critic" | "context_compressor" | "agent_component",
  handler: (
    call: ExtensionInvokeCall<I>,
    signal: AbortSignal,
  ) => Promise<R | ExtensionOutcome<R>> | R | ExtensionOutcome<R>,
): ExtensionHandler {
  return async (value, signal) => {
    const call = decodeExtensionInvokeCall(value, expectedKind) as ExtensionInvokeCall<I>;
    const result = await handler(call, signal);
    if (expectedKind === "agent_component") {
      if (call.invocation.operation === "agent_component_call_stream") {
        if (!isExtensionOutcome(result)
          || result.outcome !== "stream"
          || !isWireHandle(result.stream)
          || result.stream.kind !== "stream") {
          throw new EchoAgentError(
            "invalid_value",
            "streaming Agent component callback must return the Host-issued stream",
          );
        }
        return normalizeTypedExtensionResult(call.invocation.operation, result);
      }
      const expected = (call.invocation as AgentComponentInvocation).input.call.operation;
      const payload = isExtensionOutcome(result)
        ? result.outcome === "result" ? result.result : undefined
        : result;
      if (!isRecord(payload)
        || payload.operation !== "agent_component_call"
        || !isRecord(payload.value)
        || !isRecord(payload.value.result)
        || payload.value.result.operation !== expected) {
        throw new EchoAgentError(
          "invalid_value",
          "agent component result does not match the typed callback operation",
        );
      }
      validateAgentComponentResult(
        (call.invocation as AgentComponentInvocation).input.component,
        expected,
        payload.value.result,
      );
    }
    return normalizeTypedExtensionResult(call.invocation.operation, result);
  };
}

function normalizeTypedExtensionResult(operation: string, value: unknown): ExtensionInvokeOutcome {
  if (isExtensionOutcome(value)) {
    if (value.outcome === "result") {
      if (!isExtensionResult(value.result) || value.result.operation !== operation) {
        throw new EchoAgentError("invalid_value", "typed extension result operation does not match invocation");
      }
    } else if (value.outcome === "stream") {
      if (!isWireHandle(value.stream) || value.stream.kind !== "stream") {
        throw new EchoAgentError("invalid_value", "typed extension stream outcome has an invalid handle");
      }
      if (!operation.endsWith("_stream")) {
        throw new EchoAgentError("invalid_value", "non-streaming extension operation returned a stream");
      }
    }
    return value;
  }
  if (!isExtensionResult(value) || value.operation !== operation || operation.endsWith("_stream")) {
    throw new EchoAgentError("invalid_value", "typed extension result operation does not match invocation");
  }
  return { outcome: "result", result: value };
}

function isExtensionOutcome(value: unknown): value is ExtensionInvokeOutcome {
  return isRecord(value) && (value.outcome === "result" || value.outcome === "stream" || value.outcome === "error");
}

function isExtensionResult(value: unknown): value is ExtensionResult {
  return isRecord(value) && typeof value.operation === "string" && "value" in value;
}

function extensionKindForOperation(operation: string): "tool" | "llm_client" | "store" | "critic" | "context_compressor" | "agent_component" | undefined {
  if (operation === "tool_execute" || operation === "tool_execute_stream" || operation === "tool_validate_parameters") {
    return "tool";
  }
  if (operation === "llm_chat" || operation === "llm_chat_stream") return "llm_client";
  if (operation === "critic_critique") return "critic";
  if (operation === "compressor_compress") return "context_compressor";
  if (operation === "agent_component_call" || operation === "agent_component_call_stream") {
    return "agent_component";
  }
  if (operation.startsWith("store_")) return "store";
  return undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

const EXTENSION_ERROR_CODES = new Set([
  "acp_protocol_mismatch",
  "extension_version_mismatch",
  "extension_digest_mismatch",
  "extension_capability_mismatch",
  "invalid_request",
  "invalid_config",
  "invalid_value",
  "feature_unavailable",
  "stale_handle",
  "closed_handle",
  "framework_error",
  "extension_rejected",
  "extension_failed",
  "extension_timeout",
  "extension_disconnected",
  "extension_conflict",
  "cancelled",
  "host_shutting_down",
  "host_exited",
  "event_gap",
  "replay_unavailable",
  "payload_too_large",
  "serialization_violation",
]);

function extensionFailure(error: unknown): EchoAgentError {
  const candidate = EchoAgentError.fromUnknown(error, "_echo_agent/extension/invoke");
  if (EXTENSION_ERROR_CODES.has(candidate.code)) return candidate;
  return new EchoAgentError(
    "extension_failed",
    candidate.message,
    candidate.retryable,
    "_echo_agent/extension/invoke",
    candidate.details,
  );
}
