package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

/**
 * A versioned descriptor for one Host-language implementation.
 *
 * <p>The descriptor is only registration metadata.  Invocation and
 * settlement remain owned by the ACP Host and {@link EchoAgentClient}.</p>
 */
public interface ExtensionDescriptor {
    String kind();

    /** Returns a defensive JSON snapshot suitable for the register request. */
    ObjectNode toJson();
}
