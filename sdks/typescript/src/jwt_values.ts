/** A2A JWT configuration and claims values without owning token verification. */
export class JwtConfig {
  private constructor(
    private readonly algorithm: "HS256" | "RS256" | null,
    private readonly verificationKey: string | null,
    public readonly issuer: string | null,
    public readonly audience: string | null,
  ) {}

  public static hs256(secret: string): JwtConfig {
    if (typeof secret !== "string") throw new TypeError("JWT secret must be text");
    return new JwtConfig("HS256", secret, null, null);
  }

  public static rs256(publicKey: string): JwtConfig {
    if (typeof publicKey !== "string" || !looksLikeRsaPublicKey(publicKey)) {
      throw new Error("invalid RSA public key");
    }
    return new JwtConfig("RS256", publicKey, null, null);
  }

  public static disabled(): JwtConfig {
    return new JwtConfig(null, null, null, null);
  }

  public withIssuer(issuer: string): JwtConfig {
    if (typeof issuer !== "string") throw new TypeError("JWT issuer must be text");
    return new JwtConfig(this.algorithm, this.verificationKey, issuer, this.audience);
  }

  public withAudience(audience: string): JwtConfig {
    if (typeof audience !== "string") throw new TypeError("JWT audience must be text");
    return new JwtConfig(this.algorithm, this.verificationKey, this.issuer, audience);
  }

  public isEnabled(): boolean {
    return this.algorithm !== null;
  }

  public toString(): string {
    return `JwtConfig { enabled: ${this.isEnabled()}, algorithm: ${this.algorithm ?? "none"}, issuer: ${this.issuer ?? "null"}, audience: ${this.audience ?? "null"}, verification_key: [redacted] }`;
  }
}

export class JwtClaims {
  public constructor(
    public readonly iss: string | null = null,
    public readonly sub: string | null = null,
    public readonly aud: string | null = null,
    public readonly exp: bigint | null = null,
    public readonly nbf: bigint | null = null,
    public readonly iat: bigint | null = null,
    public readonly jti: string | null = null,
    public readonly extra: Readonly<Record<string, unknown>> = {},
  ) {}

  public subject(): string | undefined {
    return this.sub ?? undefined;
  }
}

function looksLikeRsaPublicKey(value: string): boolean {
  const trimmed = value.trim();
  return (trimmed.startsWith("-----BEGIN PUBLIC KEY-----")
      && trimmed.endsWith("-----END PUBLIC KEY-----"))
    || (trimmed.startsWith("-----BEGIN RSA PUBLIC KEY-----")
      && trimmed.endsWith("-----END RSA PUBLIC KEY-----"));
}
