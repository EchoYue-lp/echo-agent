package com.echoagent.sdk;

import com.agentclientprotocol.sdk.client.transport.AgentParameters;
import com.agentclientprotocol.sdk.client.transport.StdioAcpClientTransport;
import com.agentclientprotocol.sdk.json.AcpJsonMapper;
import com.agentclientprotocol.sdk.json.TypeRef;
import com.agentclientprotocol.sdk.spec.AcpClientSession;
import com.agentclientprotocol.sdk.spec.AcpSchema;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import reactor.core.publisher.Mono;

import java.io.IOException;
import java.nio.file.Path;
import java.time.Duration;
import java.util.List;
import java.util.Map;
import java.util.Collection;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Flow;
import java.util.function.Function;

/** Source-built ACP v1 client. The official ACP Java transport owns JSON-RPC and stdio. */
public final class EchoAgentClient implements AutoCloseable {
    private static final TypeRef<JsonNode> JSON_NODE = new TypeRef<>() {};

    private final AcpClientSession session;
    private final FacadeCatalog catalog;
    private final Map<String, BoundedPublisher> eventPublishers = new ConcurrentHashMap<>();
    private final Map<String, BoundedPublisher> updatePublishers = new ConcurrentHashMap<>();
    private final Map<String, ExtensionHandler> extensionHandlers = new ConcurrentHashMap<>();
    private final Map<String, ExtensionCancellation> invocationCancellations = new ConcurrentHashMap<>();
    private volatile boolean closed;
    private volatile JsonNode capability;

    private EchoAgentClient(AcpClientSession session, FacadeCatalog catalog) {
        this.session = session;
        this.catalog = catalog;
    }

    public static CompletionStage<EchoAgentClient> spawn(
            String hostCommand,
            List<String> args,
            Path catalogPath,
            Path digestPath,
            Map<String, String> environment) {
        return spawn(hostCommand, args, catalogPath, digestPath, environment, List.of(), List.of());
    }

    /**
     * Starts a client and fails closed when the Host lacks a requested
     * compile-time feature or negotiated extension capability.
     */
    public static CompletionStage<EchoAgentClient> spawn(
            String hostCommand,
            List<String> args,
            Path catalogPath,
            Path digestPath,
            Map<String, String> environment,
            Collection<String> requiredFeatures,
            Collection<String> requiredCapabilities) {
        try {
            AgentParameters.Builder parameters = AgentParameters.builder(hostCommand)
                    .args(args == null ? List.of() : args);
            if (environment != null && !environment.isEmpty()) parameters.env(environment);
            AcpJsonMapper mapper = AcpJsonMapper.createDefault();
            StdioAcpClientTransport transport = new StdioAcpClientTransport(parameters.build(), mapper);
            var requestHandlers = new ConcurrentHashMap<String, AcpClientSession.RequestHandler<?>>();
            var notificationHandlers = new ConcurrentHashMap<String, AcpClientSession.NotificationHandler>();
            FacadeCatalog catalog = new FacadeCatalog(catalogPath, digestPath);
            EchoAgentClient[] holder = new EchoAgentClient[1];

            requestHandlers.put("_echo_agent/extension/invoke", (AcpClientSession.RequestHandler<JsonNode>) params -> {
                JsonNode call = transport.unmarshalFrom(params, JSON_NODE);
                EchoAgentClient client = holder[0];
                String extensionId = call.path("extension").path("id").asText("");
                ExtensionHandler handler = client.extensionHandlers.get(extensionId);
                if (handler == null) return Mono.just(errorOutcome("extension registration is not available"));
                String invocationId = call.path("invocation_id").asText("");
                ExtensionCancellation cancellation = new ExtensionCancellation();
                if (!invocationId.isEmpty()) client.invocationCancellations.put(invocationId, cancellation);
                try {
                    return Mono.fromFuture(handler.handle(call, cancellation).toCompletableFuture())
                            .map(result -> cancellation.isCancelled() ? errorOutcome("extension invocation was cancelled") : normalizeOutcome(result))
                            .doFinally(ignored -> client.invocationCancellations.remove(invocationId));
                } catch (RuntimeException error) {
                    return Mono.just(errorOutcome(error.getMessage() == null ? "extension failed" : error.getMessage()));
                }
            });
            notificationHandlers.put("_echo_agent/extension/cancel", params -> {
                JsonNode notice = transport.unmarshalFrom(params, JSON_NODE);
                String invocationId = notice.path("invocation_id").asText("");
                ExtensionCancellation cancellation = holder[0].invocationCancellations.get(invocationId);
                if (cancellation != null) cancellation.cancel();
                return Mono.empty();
            });
            notificationHandlers.put("_echo_agent/event", params -> {
                JsonNode event = transport.unmarshalFrom(params, JSON_NODE);
                final WireHandle stream;
                try {
                    stream = incomingStream(event);
                } catch (RuntimeException error) {
                    holder[0].failEventPublishers(new EchoAgentException(
                            "serialization_violation", "event notification has an invalid stream handle",
                            "never", "_echo_agent/event", event));
                    return Mono.empty();
                }
                BoundedPublisher publisher = holder[0].eventPublishers
                        .computeIfAbsent(stream.id(), ignored -> holder[0].newEventPublisher(stream));
                publisher.publishEvent(event, stream.id());
                String eventType = event.path("envelope").path("payload")
                        .path("event_type").asText("");
                if ("final_answer".equals(eventType)
                        || "cancelled".equals(eventType)
                        || "error".equals(eventType)) {
                    publisher.close();
                }
                return Mono.empty();
            });
            notificationHandlers.put("_echo_agent/gap", params -> {
                JsonNode gap = transport.unmarshalFrom(params, JSON_NODE);
                final WireHandle stream;
                try {
                    stream = incomingStream(gap);
                } catch (RuntimeException error) {
                    holder[0].failEventPublishers(new EchoAgentException(
                            "serialization_violation", "gap notification has an invalid stream handle",
                            "never", "_echo_agent/gap", gap));
                    return Mono.empty();
                }
                holder[0].eventPublishers
                        .computeIfAbsent(stream.id(), ignored -> holder[0].newEventPublisher(stream))
                        .publishGap(gap, stream.id());
                return Mono.empty();
            });
            notificationHandlers.put("session/update", params -> {
                JsonNode update = transport.unmarshalFrom(params, JSON_NODE);
                String sessionId = update.path("sessionId").asText("");
                if (!sessionId.isEmpty()) {
                    holder[0].updatePublishers
                            .computeIfAbsent(sessionId, ignored -> new BoundedPublisher())
                            .publish(update);
                }
                return Mono.empty();
            });

            AcpClientSession session = new AcpClientSession(
                    Duration.ofSeconds(30), transport, requestHandlers, notificationHandlers,
                    Function.identity());
            EchoAgentClient client = new EchoAgentClient(session, catalog);
            holder[0] = client;

            ObjectNode hello = mapper.convertValue(Map.of(
                    "extension_protocol_version", 1,
                    "contract_digest", catalog.contractDigest(),
                    "source_contract_digest", catalog.sourceContractDigest(),
                    "required_features", sortedDistinct(requiredFeatures),
                    "required_capabilities", sortedDistinct(requiredCapabilities)), ObjectNode.class);
            var capabilities = new AcpSchema.ClientCapabilities(
                    new AcpSchema.FileSystemCapability(false, false), false, null,
                    Map.of("echo_agent", hello));
            var initialize = new AcpSchema.InitializeRequest(
                    1, capabilities,
                    new AcpSchema.Implementation("echo-agent-sdk-java", "0.1.0"), Map.of());
            return session.sendRequest("initialize", initialize, JSON_NODE)
                    .toFuture()
                    .thenApply(result -> {
                        JsonNode advertised = result.path("agentCapabilities")
                                .path("_meta").path("echo_agent");
                        if (!advertised.isObject()) {
                            throw new EchoAgentException(
                                    "extension_capability_mismatch",
                                    "Host did not advertise echo-agent extension",
                                    "never", "initialize", null);
                        }
                        if (!catalog.contractDigest().equals(advertised.path("contract_digest").asText())
                                || !catalog.sourceContractDigest().equals(
                                        advertised.path("source_contract_digest").asText())) {
                            throw new EchoAgentException(
                                    "extension_capability_mismatch",
                                    "Host contract digest mismatch", "never", "initialize", advertised);
                        }
                        validateCapability(advertised, requiredFeatures, requiredCapabilities);
                        client.capability = advertised;
                        return client;
                    });
        } catch (IOException | RuntimeException error) {
            return CompletableFuture.failedFuture(new EchoAgentException(
                    "transport_error", error.getMessage(), "never", null, null));
        }
    }

    private static JsonNode errorOutcome(String message) {
        ObjectNode outcome = JsonSupport.MAPPER.createObjectNode();
        outcome.put("outcome", "error");
        outcome.set("error", JsonSupport.MAPPER.createObjectNode()
                .put("code", "invalid_value")
                .put("message", message)
                .put("retryable", "never"));
        return outcome;
    }

    private static JsonNode normalizeOutcome(JsonNode value) {
        if (value == null || !value.isObject() || !value.path("outcome").isTextual()) {
            return errorOutcome("extension handler must return an outcome");
        }
        String outcome = value.path("outcome").asText();
        boolean valid = switch (outcome) {
            case "result" -> value.has("result");
            case "stream" -> value.path("stream").isObject();
            case "error" -> value.path("error").isObject();
            default -> false;
        };
        return valid ? value : errorOutcome("extension handler returned a malformed outcome");
    }

    public JsonNode capability() { return capability; }
    public FacadeCatalog catalog() { return catalog; }

    private static List<String> sortedDistinct(Collection<String> values) {
        var result = new ArrayList<String>();
        if (values != null) result.addAll(values);
        result.removeIf(value -> value == null || value.isBlank());
        result.sort(Comparator.naturalOrder());
        return result.stream().distinct().toList();
    }

    private static void validateCapability(
            JsonNode advertised, Collection<String> requiredFeatures, Collection<String> requiredCapabilities) {
        var features = new java.util.HashSet<String>();
        advertised.path("features").elements().forEachRemaining(value -> {
            if (value.isTextual()) features.add(value.textValue());
        });
        for (String required : sortedDistinct(requiredFeatures)) {
            if (!features.contains(required)) {
                throw new EchoAgentException("feature_unavailable", "Host lacks feature " + required,
                        "never", "initialize", advertised);
            }
        }
        var capabilities = new java.util.HashSet<String>();
        advertised.path("capabilities").elements().forEachRemaining(value -> {
            if (value.path("capability").isTextual()) capabilities.add(value.path("capability").textValue());
        });
        for (String required : sortedDistinct(requiredCapabilities)) {
            if (!capabilities.contains(required)) {
                throw new EchoAgentException("extension_capability_mismatch", "Host lacks capability " + required,
                        "never", "initialize", advertised);
            }
        }
    }

    public CompletionStage<JsonNode> request(String method, JsonNode params) {
        if (closed) {
            return CompletableFuture.failedFuture(new EchoAgentException(
                    "host_exited", "ACP connection is closed", "never", method, null));
        }
        return session.sendRequest(method, params, JSON_NODE).toFuture();
    }

    public CompletionStage<Void> notify(String method, JsonNode params) {
        if (closed) {
            return CompletableFuture.failedFuture(new EchoAgentException(
                    "host_exited", "ACP connection is closed", "never", method, null));
        }
        return session.sendNotification(method, params).toFuture();
    }

    public CompletionStage<JsonNode> invoke(String operation, WireHandle handle, List<?> arguments) {
        ObjectNode params = JsonSupport.MAPPER.createObjectNode();
        params.put("operation", operation);
        params.put("signature_digest", catalog.signature(operation));
        if (handle == null) params.putNull("handle");
        else params.set("handle", handle.toJson());
        var values = params.putArray("arguments");
        for (Object argument : arguments == null ? List.of() : arguments) {
            values.add(JsonSupport.wire(argument));
        }
        return request("_echo_agent/facade/invoke", params)
                .thenApply(response -> JsonSupport.fromWire(response.path("value")));
    }

    /** Count with the exact Host-owned tokenizer supplied to a compression callback. */
    public CompletionStage<java.math.BigInteger> countTokens(TokenizerReference tokenizer, String text) {
        java.util.Objects.requireNonNull(tokenizer, "tokenizer");
        java.util.Objects.requireNonNull(text, "text");
        return invoke(
                "echo_core::tokenizer::Tokenizer::count_tokens",
                tokenizer.resource(),
                List.of(tokenizer.ownerSessionId(), text))
                .thenApply(value -> new java.math.BigInteger(
                        TypedExtensionSupport.canonicalU64(value.textValue(), "token count")));
    }

    public CompletionStage<JsonNode> call(String operation, WireHandle handle, List<?> arguments) {
        var resolved = catalog.resolve(operation);
        if ("_echo_agent/facade/invoke".equals(resolved.method())) {
            return invoke(operation, handle, arguments);
        }
        return family(resolved.family(), operation, handle, arguments);
    }

    /** Delegates AgentEvent -> TurnOutcome classification to the canonical Host authority. */
    public CompletionStage<JsonNode> classifyTurnOutcome(JsonNode eventWire) {
        java.util.Objects.requireNonNull(eventWire, "eventWire");
        return call(
                "echo_orchestration::runtime::turn_driver::TurnOutcome::classify",
                null,
                List.of(eventWire));
    }

    public CompletionStage<JsonNode> family(
            String family, String operation, WireHandle sessionHandle, List<?> arguments) {
        String method = "_echo_agent/" + family + "/op";
        ObjectNode params = JsonSupport.MAPPER.createObjectNode();
        params.put("operation", operation);
        params.put("signature_digest", catalog.familySignature(method, operation));
        if (sessionHandle == null) params.putNull("handle");
        else params.set("handle", sessionHandle.toJson());
        var values = params.putArray("arguments");
        for (Object argument : arguments == null ? List.of() : arguments) {
            values.add(JsonSupport.wire(argument));
        }
        return request(method, params)
                .thenApply(response -> JsonSupport.fromWire(response.path("value")));
    }

    public CompletionStage<ExtensionRegistration> registerExtension(
            String kind, String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        ObjectNode params = JsonSupport.MAPPER.createObjectNode();
        params.put("kind", kind);
        params.put("implementation_id", implementationId);
        params.set("descriptor", descriptor);
        return request("_echo_agent/extension/register", params).thenApply(result -> {
            WireHandle wire = WireHandle.fromJson(result.path("extension"));
            extensionHandlers.put(wire.id(), handler);
            return new ExtensionRegistration(this, wire);
        });
    }

    public CompletionStage<ExtensionRegistration> registerTypedExtension(
            String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        if (descriptor == null || !descriptor.path("kind").isTextual()
                || descriptor.path("descriptor_version").asInt(-1) != 1) {
            return CompletableFuture.failedFuture(new EchoAgentException(
                    "invalid_value", "extension descriptor must declare version 1 and kind",
                    "never", "_echo_agent/extension/register", null));
        }
        return registerExtension(descriptor.path("kind").asText(), implementationId, descriptor, handler);
    }

    private CompletionStage<ExtensionRegistration> registerExtensionKind(
            String expected, String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        if (descriptor == null || !expected.equals(descriptor.path("kind").asText())) {
            return CompletableFuture.failedFuture(new EchoAgentException(
                    "invalid_value", "expected " + expected + " extension descriptor",
                    "never", "_echo_agent/extension/register", null));
        }
        return registerTypedExtension(implementationId, descriptor, handler);
    }

    public CompletionStage<ExtensionRegistration> registerTool(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("tool", implementationId, descriptor, handler);
    }
    /** Registers a Tool using the lossless typed callback facade. */
    public CompletionStage<ExtensionRegistration> registerTool(
            String implementationId, ToolDescriptor descriptor, ToolHandler handler) {
        java.util.Objects.requireNonNull(descriptor, "descriptor");
        java.util.Objects.requireNonNull(handler, "handler");
        return registerExtension("tool", implementationId, descriptor.toJson(), (call, cancellation) -> {
            try {
                var typedCall = ToolCall.from(call);
                return encodeTypedOutcome(handler.handle(typedCall, cancellation), typedCall.operation(),
                        java.util.Set.of("tool_execute", "tool_validate_parameters"));
            } catch (RuntimeException error) {
                return failedExtensionOutcome(error);
            }
        });
    }
    public CompletionStage<ExtensionRegistration> registerLlmClient(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("llm_client", implementationId, descriptor, handler);
    }
    /** Registers an LlmClient using the lossless typed callback facade. */
    public CompletionStage<ExtensionRegistration> registerLlmClient(
            String implementationId, LlmClientDescriptor descriptor, LlmClientHandler handler) {
        java.util.Objects.requireNonNull(descriptor, "descriptor");
        java.util.Objects.requireNonNull(handler, "handler");
        return registerExtension("llm_client", implementationId, descriptor.toJson(), (call, cancellation) -> {
            try {
                var typedCall = LlmChatCall.from(call);
                return encodeTypedOutcome(handler.handle(typedCall, cancellation), typedCall.operation(),
                        java.util.Set.of("llm_chat"));
            } catch (RuntimeException error) {
                return failedExtensionOutcome(error);
            }
        });
    }
    public CompletionStage<ExtensionRegistration> registerStore(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("store", implementationId, descriptor, handler);
    }
    /** Registers a Store using the lossless typed callback facade. */
    public CompletionStage<ExtensionRegistration> registerStore(
            String implementationId, StoreDescriptor descriptor, StoreHandler handler) {
        java.util.Objects.requireNonNull(descriptor, "descriptor");
        java.util.Objects.requireNonNull(handler, "handler");
        return registerExtension("store", implementationId, descriptor.toJson(), (call, cancellation) -> {
            try {
                var typedCall = StoreCall.from(call);
                return encodeTypedOutcome(handler.handle(typedCall, cancellation), typedCall.operation(),
                        java.util.Set.of("store_put", "store_get", "store_search", "store_search_with",
                                "store_delete", "store_list_namespaces", "store_list",
                                "store_prune_expired", "store_dedup_by_content"));
            } catch (RuntimeException error) {
                return failedExtensionOutcome(error);
            }
        });
    }
    public CompletionStage<ExtensionRegistration> registerCritic(
            String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("critic", implementationId, descriptor, handler);
    }
    /** Registers a Critic using the lossless typed callback facade. */
    public CompletionStage<ExtensionRegistration> registerCritic(
            String implementationId, CriticDescriptor descriptor, CriticHandler handler) {
        java.util.Objects.requireNonNull(descriptor, "descriptor");
        java.util.Objects.requireNonNull(handler, "handler");
        return registerExtension("critic", implementationId, descriptor.toJson(), (call, cancellation) -> {
            try {
                var typedCall = CriticCall.from(call);
                return encodeTypedOutcome(handler.handle(typedCall, cancellation), typedCall.operation(),
                        java.util.Set.of("critic_critique"));
            } catch (RuntimeException error) {
                return failedExtensionOutcome(error);
            }
        });
    }
    /** Registers a ContextCompressor using the lossless typed callback facade. */
    public CompletionStage<ExtensionRegistration> registerContextCompressor(
            String implementationId, ContextCompressorDescriptor descriptor, CompressionHandler handler) {
        java.util.Objects.requireNonNull(descriptor, "descriptor");
        java.util.Objects.requireNonNull(handler, "handler");
        return registerExtension(descriptor.kind(), implementationId, descriptor.toJson(), (call, cancellation) -> {
            try {
                var typedCall = CompressionCall.from(call);
                return encodeTypedOutcome(handler.handle(typedCall, cancellation), typedCall.operation(),
                        java.util.Set.of("compressor_compress"));
            } catch (RuntimeException error) {
                return failedExtensionOutcome(error);
            }
        });
    }
    /** Registers a live-bindable Agent infrastructure component. */
    public CompletionStage<ExtensionRegistration> registerAgentComponent(
            String implementationId, AgentComponentDescriptor descriptor, AgentComponentHandler handler) {
        java.util.Objects.requireNonNull(descriptor, "descriptor");
        java.util.Objects.requireNonNull(handler, "handler");
        return registerExtension(descriptor.kind(), implementationId, descriptor.toJson(), (call, cancellation) -> {
            try {
                var typedCall = AgentComponentCall.from(call);
                return encodeTypedOutcome(handler.handle(typedCall, cancellation), typedCall.operation(),
                        java.util.Set.of("agent_component_call", "agent_component_call_stream"));
            } catch (RuntimeException error) {
                return failedExtensionOutcome(error);
            }
        });
    }
    public CompletionStage<ExtensionRegistration> registerHumanLoopProvider(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("human_loop_provider", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerHook(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("hook", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerAgentCallback(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("agent_callback", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerInterventionCallback(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("intervention_callback", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerAgentFactory(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("agent_factory", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerCustomAgent(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("custom_agent", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerChannelPlugin(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("channel_plugin", implementationId, descriptor, handler);
    }
    public CompletionStage<ExtensionRegistration> registerChannelMessageHandler(String implementationId, JsonNode descriptor, ExtensionHandler handler) {
        return registerExtensionKind("channel_message_handler", implementationId, descriptor, handler);
    }

    private static CompletionStage<JsonNode> encodeTypedOutcome(
            CompletionStage<? extends ExtensionOutcome> outcome, String invocationOperation,
            java.util.Set<String> operations) {
        if (outcome == null) {
            return CompletableFuture.failedFuture(new IllegalArgumentException(
                    "typed extension handler must return a CompletionStage"));
        }
        return outcome.handle((value, failure) -> {
            if (failure != null) {
                return extensionFailureJson(failure);
            }
            if (value == null) throw new IllegalArgumentException("typed extension outcome is required");
            var json = value.toJson();
            if (json == null || !json.isObject()) {
                throw new IllegalArgumentException("typed extension outcome must be an object");
            }
            var outcomeKind = json.path("outcome").asText("");
            var operation = json.path("result").path("operation");
            if (operation.isTextual() && !operations.contains(operation.textValue())) {
                throw new IllegalArgumentException("typed extension outcome operation is not valid for this extension");
            }
            if ("result".equals(outcomeKind)
                    && (!operation.isTextual() || !invocationOperation.equals(operation.textValue()))) {
                throw new IllegalArgumentException("typed extension result operation does not match invocation");
            }
            if ("stream".equals(outcomeKind) && !invocationOperation.endsWith("_stream")) {
                throw new IllegalArgumentException("non-streaming extension operation returned a stream");
            }
            return json;
        });
    }

    private static CompletionStage<JsonNode> failedExtensionOutcome(RuntimeException error) {
        return CompletableFuture.completedFuture(extensionFailureJson(error));
    }

    private static JsonNode extensionFailureJson(Throwable error) {
        var message = error.getMessage() == null ? "typed extension handler failed" : error.getMessage();
        return ExtensionOutcome.error("extension_failed", message, "never").toJson();
    }

    CompletionStage<JsonNode> unregisterExtension(WireHandle extension) {
        extensionHandlers.remove(extension.id());
        return request(
                "_echo_agent/extension/unregister",
                JsonSupport.MAPPER.createObjectNode().set("extension", extension.toJson()));
    }

    public CompletionStage<AgentHandle> createAgent() {
        ObjectNode params = JsonSupport.MAPPER.createObjectNode();
        params.putObject("config").put("variant", "host_default");
        return request("_echo_agent/agent/create", params)
                .thenApply(result -> new AgentHandle(
                        this, WireHandle.fromJson(result.path("agent"))));
    }

    Flow.Publisher<JsonNode> events(WireHandle stream) {
        BoundedPublisher publisher = eventPublishers.computeIfAbsent(stream.id(), ignored -> newEventPublisher(stream));
        publisher.validateExpectedStream(stream);
        if (closed) publisher.close();
        return publisher;
    }

    private BoundedPublisher newEventPublisher(WireHandle stream) {
        return new BoundedPublisher(BoundedPublisher.DEFAULT_CAPACITY,
                event -> acknowledgeEvent(stream.toJson(), event), stream);
    }

    private static WireHandle incomingStream(JsonNode value) {
        if (value == null || !value.isObject()) {
            throw new IllegalArgumentException("notification must be an object");
        }
        WireHandle stream = WireHandle.fromJson(value.path("stream"));
        if (!"stream".equals(stream.kind())) {
            throw new IllegalArgumentException("notification requires a stream handle");
        }
        return stream;
    }

    private void failEventPublishers(EchoAgentException error) {
        eventPublishers.values().forEach(publisher -> publisher.fail(error));
    }

    private void acknowledgeEvent(JsonNode stream, JsonNode event) {
        String sequence = event.path("envelope").path("sequence").asText("");
        if (sequence.isEmpty()) sequence = event.path("gap").path("snapshot_watermark").asText("");
        if (!sequence.matches("[1-9][0-9]*")) return;
        ObjectNode params = JsonSupport.MAPPER.createObjectNode();
        ObjectNode ack = JsonSupport.MAPPER.createObjectNode();
        ack.set("stream", stream);
        ack.put("last_processed_sequence", sequence);
        params.set("ack", ack);
        notify("_echo_agent/event/ack", params).exceptionally(ignored -> null);
    }

    Flow.Publisher<JsonNode> updates(String sessionId) {
        BoundedPublisher publisher = updatePublishers.computeIfAbsent(sessionId, ignored -> new BoundedPublisher());
        if (closed) publisher.close();
        return publisher;
    }

    void publishFirstEvent(WireHandle stream, JsonNode envelope) {
        if (envelope == null || envelope.isNull()) return;
        ObjectNode event = JsonSupport.MAPPER.createObjectNode();
        event.set("stream", stream.toJson());
        event.set("envelope", envelope);
        eventPublishers.computeIfAbsent(stream.id(), ignored -> newEventPublisher(stream))
                .publishEvent(event, stream.id());
        String eventType = envelope.path("payload").path("event_type").asText("");
        if ("final_answer".equals(eventType)
                || "cancelled".equals(eventType)
                || "error".equals(eventType)) {
            eventPublishers.get(stream.id()).close();
        }
    }

    void closeEvents(WireHandle stream) {
        BoundedPublisher publisher = eventPublishers.get(stream.id());
        if (publisher != null) publisher.close();
    }

    void closeUpdates(String sessionId) {
        BoundedPublisher publisher = updatePublishers.get(sessionId);
        if (publisher != null) publisher.close();
    }

    public ExtensionStreamWriter streamWriter(WireHandle stream) {
        return new ExtensionStreamWriter(this, stream);
    }

    @Override
    public void close() {
        closeAsync().toCompletableFuture().join();
    }

    /**
     * Closes the ACP session and all local publishers with a five-second
     * bound. The returned stage is safe to await from asynchronous callers;
     * {@link #close()} is the blocking AutoCloseable convenience.
     */
    public CompletionStage<Void> closeAsync() {
        if (closed) return CompletableFuture.completedFuture(null);
        closed = true;
        extensionHandlers.clear();
        invocationCancellations.values().forEach(ExtensionCancellation::cancel);
        invocationCancellations.clear();
        eventPublishers.values().forEach(BoundedPublisher::close);
        updatePublishers.values().forEach(BoundedPublisher::close);
        return session.closeGracefully()
                .timeout(Duration.ofSeconds(5))
                .onErrorResume(ignored -> Mono.empty())
                .toFuture();
    }
}
