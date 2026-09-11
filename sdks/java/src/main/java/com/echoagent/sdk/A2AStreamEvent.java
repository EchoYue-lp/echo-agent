package com.echoagent.sdk;

/** Closed A2A stream event union. */
public sealed interface A2AStreamEvent permits A2AStreamEvent.StatusUpdate, A2AStreamEvent.ArtifactUpdate {
    record StatusUpdate(TaskStatusUpdateEvent event) implements A2AStreamEvent {
        public StatusUpdate {
            if (event == null) throw new IllegalArgumentException("status event must not be null");
        }
    }

    record ArtifactUpdate(TaskArtifactUpdateEvent event) implements A2AStreamEvent {
        public ArtifactUpdate {
            if (event == null) throw new IllegalArgumentException("artifact event must not be null");
        }
    }
}
