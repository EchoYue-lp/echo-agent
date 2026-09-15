package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Flow;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertTrue;

class LifecycleTest {
    @Test
    void publisherRetainsPreSubscriptionValuesWithinBound() throws Exception {
        var publisher = new BoundedPublisher(2);
        publisher.publish(JsonSupport.MAPPER.createObjectNode().put("value", 1));
        publisher.publish(JsonSupport.MAPPER.createObjectNode().put("value", 2));
        var subscriber = new RecordingSubscriber(2);
        publisher.subscribe(subscriber);

        assertTrue(subscriber.values.await(2, TimeUnit.SECONDS), () -> "event error=" + subscriber.error.get());
        assertEquals(List.of(1, 2), subscriber.valuesSnapshot().stream()
                .map(value -> value.path("value").asInt()).toList());
        publisher.close();
        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
    }

    @Test
    void publisherFailsExplicitlyWhenPreSubscriptionBoundIsExceeded() throws Exception {
        var publisher = new BoundedPublisher(2);
        publisher.publish(JsonSupport.MAPPER.createObjectNode().put("value", 1));
        publisher.publish(JsonSupport.MAPPER.createObjectNode().put("value", 2));
        publisher.publish(JsonSupport.MAPPER.createObjectNode().put("value", 3));

        assertTrue(publisher.isClosed());
        var subscriber = new RecordingSubscriber(0);
        publisher.subscribe(subscriber);
        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertInstanceOf(EchoAgentException.class, subscriber.error.get());
        assertEquals("event_gap", ((EchoAgentException) subscriber.error.get()).code());
    }

    @Test
    void eventPublisherRejectsGapsAndIgnoresDuplicateDelivery() throws Exception {
        var publisher = new BoundedPublisher(8);
        var stream = new WireHandle("stream-1", "1", "stream");
        var subscriber = new RecordingSubscriber(1);
        publisher.subscribe(subscriber);
        publisher.publishEvent(event(stream, "1"), stream.id());
        publisher.publishEvent(event(stream, "1"), stream.id());
        assertTrue(subscriber.values.await(2, TimeUnit.SECONDS), () -> "event error=" + subscriber.error.get());
        assertEquals(1, subscriber.valuesSnapshot().size());

        publisher.publishEvent(event(stream, "3"), stream.id());
        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertInstanceOf(EchoAgentException.class, subscriber.error.get());
        assertEquals("event_gap", ((EchoAgentException) subscriber.error.get()).code());
    }

    @Test
    void eventPublisherRejectsAStaleGenerationBeforeAdvancing() throws Exception {
        var publisher = new BoundedPublisher(8);
        var current = new WireHandle("stream-1", "1", "stream");
        var stale = new WireHandle("stream-1", "2", "stream");
        var subscriber = new RecordingSubscriber(0);
        publisher.subscribe(subscriber);

        publisher.publishEvent(event(current, "1"), current.id());
        publisher.publishEvent(event(stale, "2"), current.id());

        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertTrue(subscriber.valuesSnapshot().size() <= 1);
        assertEquals(java.math.BigInteger.ONE, publisher.lastSequence());
        assertInstanceOf(EchoAgentException.class, subscriber.error.get());
        assertEquals("handle_mismatch", ((EchoAgentException) subscriber.error.get()).code());
    }

    @Test
    void gapValidationRejectsAnInvalidRangeWithoutPublishingOrAdvancing() throws Exception {
        var publisher = new BoundedPublisher(8);
        var stream = new WireHandle("stream-1", "1", "stream");
        var subscriber = new RecordingSubscriber(0);
        publisher.subscribe(subscriber);

        publisher.publishEvent(event(stream, "1"), stream.id());
        publisher.publishGap(gap(stream, "2", "3", "2"), stream.id());

        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertTrue(subscriber.valuesSnapshot().size() <= 1);
        assertInstanceOf(EchoAgentException.class, subscriber.error.get());
        assertEquals("serialization_violation", ((EchoAgentException) subscriber.error.get()).code());
        assertEquals(java.math.BigInteger.ONE, publisher.lastSequence());
        assertTrue(publisher.isClosed());
    }

    @Test
    void gapStartMustContinueFromTheCurrentCursor() throws Exception {
        var publisher = new BoundedPublisher(8);
        var stream = new WireHandle("stream-1", "1", "stream");
        var subscriber = new RecordingSubscriber(0);
        publisher.subscribe(subscriber);

        publisher.publishEvent(event(stream, "1"), stream.id());
        publisher.publishGap(gap(stream, "3", "3", "3"), stream.id());

        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertTrue(publisher.isClosed());
        assertEquals(java.math.BigInteger.ONE, publisher.lastSequence());
    }

    @Test
    void validGapAdvancesSequenceToItsWatermark() throws Exception {
        var publisher = new BoundedPublisher(8);
        var stream = new WireHandle("stream-1", "1", "stream");
        var subscriber = new RecordingSubscriber(3);
        publisher.subscribe(subscriber);

        publisher.publishEvent(event(stream, "1"), stream.id());
        publisher.publishGap(gap(stream, "2", "3", "3"), stream.id());
        publisher.publishGap(gap(stream, "2", "3", "3"), stream.id());
        publisher.publishEvent(event(stream, "4"), stream.id());

        assertTrue(subscriber.values.await(2, TimeUnit.SECONDS), () -> "event error=" + subscriber.error.get());
        assertEquals(3, subscriber.valuesSnapshot().size());
        assertFalse(publisher.isClosed());
        publisher.close();
    }

    @Test
    void staleGenerationGapIsRejectedBeforeAdvancing() throws Exception {
        var publisher = new BoundedPublisher(8);
        var current = new WireHandle("stream-1", "1", "stream");
        var stale = new WireHandle("stream-1", "2", "stream");
        var subscriber = new RecordingSubscriber(0);
        publisher.subscribe(subscriber);

        publisher.publishEvent(event(current, "1"), current.id());
        publisher.publishGap(gap(stale, "2", "2", "2"), current.id());

        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertTrue(publisher.isClosed());
        assertTrue(subscriber.valuesSnapshot().size() <= 1);
        assertEquals(java.math.BigInteger.ONE, publisher.lastSequence());
    }

    @Test
    void malformedGapHandlesAreRejectedBeforeAdvancing() throws Exception {
        var current = new WireHandle("stream-1", "1", "stream");
        var wrongKind = current.toJson().put("kind", "run");
        var invalidGeneration = current.toJson().put("generation", "01");

        for (JsonNode invalidStream : List.of(wrongKind, invalidGeneration)) {
            var publisher = new BoundedPublisher(8);
            publisher.publishEvent(event(current, "1"), current.id());
            JsonNode invalidGap = gap(current, "2", "2", "2");
            ((com.fasterxml.jackson.databind.node.ObjectNode) invalidGap)
                    .set("stream", invalidStream);

            publisher.publishGap(invalidGap, current.id());

            assertTrue(publisher.isClosed());
            assertEquals(java.math.BigInteger.ONE, publisher.lastSequence());
        }
    }

    @Test
    void subscriberFailurePreventsAcknowledgement() throws Exception {
        var acknowledgements = new java.util.concurrent.atomic.AtomicInteger();
        var deliveryAttempted = new CountDownLatch(1);
        var publisher = new BoundedPublisher(8, ignored -> acknowledgements.incrementAndGet());
        var stream = new WireHandle("stream-1", "1", "stream");
        publisher.subscribe(new Flow.Subscriber<>() {
            @Override public void onSubscribe(Flow.Subscription subscription) { subscription.request(1); }
            @Override public void onNext(JsonNode item) {
                deliveryAttempted.countDown();
                throw new IllegalStateException("consumer failed");
            }
            @Override public void onError(Throwable throwable) { }
            @Override public void onComplete() { }
        });

        publisher.publishEvent(event(stream, "1"), stream.id());
        assertTrue(deliveryAttempted.await(2, TimeUnit.SECONDS));
        assertEquals(0, acknowledgements.get());
        publisher.close();
    }

    @Test
    void subscriberHandleRejectsAFeedCreatedByAnOlderGeneration() throws Exception {
        var stale = new WireHandle("stream-1", "1", "stream");
        var current = new WireHandle("stream-1", "2", "stream");
        var publisher = new BoundedPublisher(8, ignored -> {}, stale);
        publisher.publishEvent(event(stale, "1"), stale.id());

        assertFalse(publisher.validateExpectedStream(current));
        var subscriber = new RecordingSubscriber(0);
        publisher.subscribe(subscriber);

        assertTrue(subscriber.terminated.await(2, TimeUnit.SECONDS));
        assertTrue(subscriber.valuesSnapshot().isEmpty());
        assertInstanceOf(EchoAgentException.class, subscriber.error.get());
        assertEquals("handle_mismatch", ((EchoAgentException) subscriber.error.get()).code());
    }

    private static JsonNode event(WireHandle stream, String sequence) {
        var envelope = JsonSupport.MAPPER.createObjectNode();
        envelope.put("stream_id", stream.id());
        envelope.put("sequence", sequence);
        envelope.set("payload", JsonSupport.MAPPER.createObjectNode().put("event_type", "token"));
        var event = JsonSupport.MAPPER.createObjectNode();
        event.set("stream", stream.toJson());
        event.set("envelope", envelope);
        return event;
    }

    private static JsonNode gap(WireHandle stream, String from, String to, String watermark) {
        var gap = JsonSupport.MAPPER.createObjectNode();
        gap.put("from_sequence", from);
        gap.put("to_sequence", to);
        gap.put("reason", "retention floor");
        gap.put("snapshot_watermark", watermark);
        var value = JsonSupport.MAPPER.createObjectNode();
        value.set("stream", stream.toJson());
        value.set("gap", gap);
        return value;
    }

    private static final class RecordingSubscriber implements Flow.Subscriber<JsonNode> {
        private final CountDownLatch values;
        private final CountDownLatch terminated = new CountDownLatch(1);
        private final CopyOnWriteArrayList<JsonNode> received = new CopyOnWriteArrayList<>();
        private final AtomicReference<Throwable> error = new AtomicReference<>();
        private final long expected;

        private RecordingSubscriber(long expected) {
            this.expected = expected;
            this.values = new CountDownLatch((int) expected);
        }

        @Override public void onSubscribe(Flow.Subscription subscription) {
            subscription.request(Long.MAX_VALUE);
        }

        @Override public void onNext(JsonNode item) {
            received.add(item);
            values.countDown();
        }

        @Override public void onError(Throwable throwable) {
            error.set(throwable);
            terminated.countDown();
        }

        @Override public void onComplete() { terminated.countDown(); }

        private List<JsonNode> valuesSnapshot() { return new ArrayList<>(received); }
    }
}
