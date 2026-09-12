package com.echoagent.sdk;

/** A2A JWT configuration projected without owning token verification. */
public final class JwtConfig {
    private final String algorithm;
    private final String verificationKey;
    private final String issuer;
    private final String audience;

    private JwtConfig(String algorithm, String verificationKey, String issuer, String audience) {
        this.algorithm = algorithm;
        this.verificationKey = verificationKey;
        this.issuer = issuer;
        this.audience = audience;
    }

    public static JwtConfig hs256(String secret) {
        if (secret == null) throw new IllegalArgumentException("JWT secret must be text");
        return new JwtConfig("HS256", secret, null, null);
    }

    public static JwtConfig rs256(String publicKey) {
        if (publicKey == null || !looksLikeRsaPublicKey(publicKey)) {
            throw new IllegalArgumentException("invalid RSA public key");
        }
        return new JwtConfig("RS256", publicKey, null, null);
    }

    public static JwtConfig disabled() {
        return new JwtConfig(null, null, null, null);
    }

    public JwtConfig withIssuer(String value) {
        if (value == null) throw new IllegalArgumentException("JWT issuer must be text");
        return new JwtConfig(algorithm, verificationKey, value, audience);
    }

    public JwtConfig withAudience(String value) {
        if (value == null) throw new IllegalArgumentException("JWT audience must be text");
        return new JwtConfig(algorithm, verificationKey, issuer, value);
    }

    public boolean isEnabled() { return algorithm != null; }

    @Override
    public String toString() {
        return "JwtConfig { enabled: " + isEnabled() + ", algorithm: "
                + (algorithm == null ? "none" : algorithm) + ", issuer: " + issuer
                + ", audience: " + audience + ", verification_key: [redacted] }";
    }

    private static boolean looksLikeRsaPublicKey(String value) {
        String trimmed = value.trim();
        return (trimmed.startsWith("-----BEGIN PUBLIC KEY-----")
                && trimmed.endsWith("-----END PUBLIC KEY-----"))
                || (trimmed.startsWith("-----BEGIN RSA PUBLIC KEY-----")
                && trimmed.endsWith("-----END RSA PUBLIC KEY-----"));
    }
}
