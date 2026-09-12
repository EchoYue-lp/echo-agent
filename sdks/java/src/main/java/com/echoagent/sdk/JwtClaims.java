package com.echoagent.sdk;

import java.math.BigInteger;
import java.util.Map;

/** A2A JWT claims projection; token verification remains Host-owned. */
public record JwtClaims(
        String iss,
        String sub,
        String aud,
        BigInteger exp,
        BigInteger nbf,
        BigInteger iat,
        String jti,
        Map<String, Object> extra) {
    public String subject() { return sub; }
}
