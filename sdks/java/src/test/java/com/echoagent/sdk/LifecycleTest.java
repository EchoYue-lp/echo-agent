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
