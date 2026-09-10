package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Collection;
import java.util.List;
import java.util.Objects;

/** Lossless builder for the wire {@code Critique} value. */
public final class Critique {
    private final ObjectNode json;

    private Critique(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }
    public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private double score;
        private boolean passed;
        private String feedback;
        private List<String> suggestions = List.of();

        public Builder score(double value) {
            if (!Double.isFinite(value) || value < 0.0 || value > 10.0) {
                throw new IllegalArgumentException("score must be finite and within 0..10");
            }
            score = value;
            return this;
        }

        public Builder passed(boolean value) { passed = value; return this; }
        public Builder feedback(String value) { feedback = value; return this; }

        public Builder suggestions(Collection<String> values) {
            suggestions = List.copyOf(values == null ? List.of() : values);
            return this;
        }

        public Critique build() {
            var result = JsonSupport.MAPPER.createObjectNode();
            result.put("score", score);
            result.put("passed", passed);
            result.put("feedback", boundedText(feedback, "feedback", 65_536));
            var encodedSuggestions = result.putArray("suggestions");
            for (String suggestion : suggestions) {
                encodedSuggestions.add(boundedText(suggestion, "suggestion", 4_096));
            }
            return new Critique(result);
        }

        private static String boundedText(String value, String name, int maxCodePoints) {
            Objects.requireNonNull(value, name);
            if (value.codePointCount(0, value.length()) > maxCodePoints) {
                throw new IllegalArgumentException(name + " exceeds its text bound");
            }
            return value;
        }
    }
}
