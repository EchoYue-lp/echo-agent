package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.time.Duration;
import java.util.ArrayDeque;
import java.util.Objects;
import java.math.BigInteger;
import java.util.concurrent.Flow;
import java.util.concurrent.ForkJoinPool;
import java.util.concurrent.SubmissionPublisher;
import java.util.concurrent.TimeUnit;
import java.util.function.Consumer;

/**
 * A bounded notification publisher which never silently drops a value.
 *
 * Values received before the first subscriber are retained up to the same
 * bound. Once a subscriber is present, a full per-subscriber buffer closes
 * the publisher exceptionally instead of allowing unbounded memory growth or
 * pretending that an event was consumed. Consumers can then use the run
 * replay API from their last acknowledged sequence.
 */
final class BoundedPublisher implements Flow.Publisher<JsonNode>, AutoCloseable {
    static final int DEFAULT_CAPACITY = 256;
    // Give a newly attached subscriber a small scheduling window to publish
    // its demand, while keeping notification dispatch bounded.
    private static final Duration OFFER_TIMEOUT = Duration.ofMillis(100);

    private final SubmissionPublisher<JsonNode> delegate;
    private final ArrayDeque<JsonNode> pending = new ArrayDeque<>();
    private final int capacity;
    private final Consumer<JsonNode> onConsumed;
    private WireHandle expectedStream;
    private BigInteger lastSequence = BigInteger.ZERO;
    private boolean closed;

    BoundedPublisher() {
        this(DEFAULT_CAPACITY);
    }

    BoundedPublisher(int capacity) {
        this(capacity, ignored -> {});
    }

    BoundedPublisher(int capacity, Consumer<JsonNode> onConsumed) {
        if (capacity < 1) throw new IllegalArgumentException("publisher capacity must be positive");
        this.onConsumed = Objects.requireNonNull(onConsumed, "onConsumed");
        this.capacity = capacity;
        this.delegate = new SubmissionPublisher<>(ForkJoinPool.commonPool(), capacity);
    }

    BoundedPublisher(int capacity, Consumer<JsonNode> onConsumed, WireHandle expectedStream) {
        this(capacity, onConsumed);
        if (expectedStream != null && !"stream".equals(expectedStream.kind())) {
            throw new IllegalArgumentException("event publisher requires a stream handle");
        }
        this.expectedStream = expectedStream;
    }

    synchronized boolean validateExpectedStream(WireHandle stream) {
        Objects.requireNonNull(stream, "stream");
        if (!"stream".equals(stream.kind())) {
            fail(new EchoAgentException("serialization_violation", "event publisher requires a stream handle",
                    "never", "_echo_agent/event", stream.toJson()));
            return false;
        }
        if (expectedStream == null) expectedStream = stream;
        if (!expectedStream.equals(stream)) {
            fail(new EchoAgentException("handle_mismatch", "event stream handle changed generation",
                    "never", "_echo_agent/event", stream.toJson()));
            return false;
        }
        return true;
    }

    @Override
    public void subscribe(Flow.Subscriber<? super JsonNode> subscriber) {
        Objects.requireNonNull(subscriber, "subscriber");
        synchronized (this) {
            if (closed) {
            delegate.subscribe(wrapping(subscriber));
            return;
        }
            delegate.subscribe(wrapping(subscriber));
            while (!pending.isEmpty() && !closed) {
                JsonNode value = pending.removeFirst();
                if (!offer(value)) break;
            }
        }
    }

    private Flow.Subscriber<JsonNode> wrapping(Flow.Subscriber<? super JsonNode> subscriber) {
        return new Flow.Subscriber<>() {
            @Override public void onSubscribe(Flow.Subscription subscription) {
                subscriber.onSubscribe(subscription);
            }
            @Override public void onNext(JsonNode item) {
                subscriber.onNext(item);
                onConsumed.accept(item);
            }
            @Override public void onError(Throwable throwable) { subscriber.onError(throwable); }
            @Override public void onComplete() { subscriber.onComplete(); }
        };
    }

    synchronized void publish(JsonNode value) {
        Objects.requireNonNull(value, "value");
        if (closed) return;
        if (!delegate.hasSubscribers()) {
            if (pending.size() >= capacity) {
                fail(new EchoAgentException(
                        "event_gap",
                        "publisher buffer is full; replay from the last acknowledged cursor",
                        "after_delay", "_echo_agent/event", null));
                return;
            }
            pending.addLast(value);
            return;
        }
        offer(value);
    }

    /** Publish one full event envelope, enforcing stream identity and order. */
    synchronized void publishEvent(JsonNode value, String expectedStreamId) {
        if (value == null || !value.isObject()) {
            fail(new EchoAgentException("serialization_violation", "event must be an object",
                    "never", "_echo_agent/event", null));
            return;
        }
        JsonNode stream = value.path("stream");
        JsonNode envelope = value.path("envelope");
        final WireHandle actualStream;
        try {
            actualStream = WireHandle.fromJson(stream);
        } catch (RuntimeException error) {
            fail(new EchoAgentException("serialization_violation", "event stream handle is malformed",
                    "never", "_echo_agent/event", value));
            return;
        }
        if (!"stream".equals(actualStream.kind())) {
            fail(new EchoAgentException("serialization_violation", "event stream handle kind is invalid",
                    "never", "_echo_agent/event", value));
            return;
        }
        if (!validateExpectedStream(actualStream)) return;
        String streamId = actualStream.id();
        String envelopeStreamId = envelope.path("stream_id").asText("");
        String sequenceText = envelope.path("sequence").isTextual()
                ? envelope.path("sequence").textValue() : "";
        if (streamId.isEmpty() || !streamId.equals(expectedStreamId)
                || !streamId.equals(envelopeStreamId)) {
            fail(new EchoAgentException("serialization_violation", "event stream identity does not match",
                    "never", "_echo_agent/event", value));
            return;
        }
        final BigInteger sequence;
        try {
            // WireValues performs canonical u64 validation; parse the text
            // directly because Jackson's TextNode#bigIntegerValue defaults
            // to zero instead of converting textual content.
            WireValues.u64(sequenceText);
            sequence = new BigInteger(sequenceText);
        } catch (RuntimeException error) {
            fail(new EchoAgentException("serialization_violation", "event sequence is not canonical u64",
                    "never", "_echo_agent/event", value));
            return;
        }
        if (sequence.signum() == 0) {
            fail(new EchoAgentException("serialization_violation", "event sequence must be positive",
                    "never", "_echo_agent/event", value));
            return;
        }
        if (lastSequence.signum() != 0) {
            BigInteger expected = lastSequence.add(BigInteger.ONE);
            if (sequence.equals(lastSequence)) return; // duplicate delivery is idempotent
            if (!sequence.equals(expected)) {
                fail(new EchoAgentException("event_gap", "event sequence is not contiguous; replay is required",
                        "after_delay", "_echo_agent/event", value));
                return;
            }
        }
        lastSequence = sequence;
        publish(value);
    }

    /** Publish a typed gap and move the contiguous cursor to its watermark. */
    synchronized void publishGap(JsonNode value, String expectedStreamId) {
        if (value == null || !value.isObject()) {
            fail(new EchoAgentException("serialization_violation", "gap must be an object",
                    "never", "_echo_agent/gap", null));
            return;
        }
        final WireHandle actualStream;
        try {
            actualStream = WireHandle.fromJson(value.path("stream"));
        } catch (RuntimeException error) {
            fail(new EchoAgentException("serialization_violation", "gap stream handle is malformed",
                    "never", "_echo_agent/gap", value));
            return;
        }
        if (!"stream".equals(actualStream.kind())) {
            fail(new EchoAgentException("serialization_violation", "gap stream handle kind is invalid",
                    "never", "_echo_agent/gap", value));
            return;
        }
        if (!validateExpectedStream(actualStream)) return;
        String streamId = actualStream.id();
        JsonNode gap = value.path("gap");
        String from = gap.path("from_sequence").isTextual()
                ? gap.path("from_sequence").textValue() : "";
        String to = gap.path("to_sequence").isTextual()
                ? gap.path("to_sequence").textValue() : "";
        String watermark = gap.path("snapshot_watermark").isTextual()
                ? gap.path("snapshot_watermark").textValue() : "";
        String reason = gap.path("reason").asText("");
        if (!streamId.equals(expectedStreamId) || reason.isBlank()
                || !from.matches("[1-9][0-9]*") || !to.matches("[1-9][0-9]*")
                || !watermark.matches("[1-9][0-9]*")) {
            fail(new EchoAgentException("serialization_violation", "gap identity or range is malformed",
                    "never", "_echo_agent/gap", value));
            return;
        }
        try {
            WireValues.u64(from);
            WireValues.u64(to);
            WireValues.u64(watermark);
            BigInteger fromValue = new BigInteger(from);
            BigInteger toValue = new BigInteger(to);
            BigInteger watermarkValue = new BigInteger(watermark);
            if (toValue.compareTo(fromValue) < 0
                    || watermarkValue.compareTo(toValue) < 0
                    || watermarkValue.compareTo(lastSequence) < 0
                    || (watermarkValue.compareTo(lastSequence) > 0
                        && lastSequence.signum() > 0
                        && !fromValue.equals(lastSequence.add(BigInteger.ONE)))) {
                fail(new EchoAgentException("serialization_violation", "gap sequence range is malformed",
                        "never", "_echo_agent/gap", value));
                return;
            }
            if (watermarkValue.equals(lastSequence)) return;
            lastSequence = watermarkValue;
        } catch (RuntimeException error) {
            fail(new EchoAgentException("serialization_violation", "gap watermark is not canonical u64",
                    "never", "_echo_agent/gap", value));
            return;
        }
        publish(value);
    }

    private boolean offer(JsonNode value) {
        int lag = delegate.offer(value, OFFER_TIMEOUT.toNanos(), TimeUnit.NANOSECONDS,
                (subscriber, dropped) -> true);
        if (lag < 0) {
            fail(new EchoAgentException(
                    "event_gap",
                    "subscriber demand exceeded the bounded publisher buffer; replay from the last acknowledged cursor",
                    "after_delay", "_echo_agent/event", null));
            return false;
        }
        return true;
    }

    synchronized void fail(Throwable error) {
        if (closed) return;
        closed = true;
        pending.clear();
        delegate.closeExceptionally(error);
    }

    synchronized boolean isClosed() {
        return closed;
    }

    synchronized BigInteger lastSequence() {
        return lastSequence;
    }

    int capacity() {
        return capacity;
    }

    @Override
    public synchronized void close() {
        if (closed) return;
        closed = true;
        pending.clear();
        delegate.close();
    }
}
