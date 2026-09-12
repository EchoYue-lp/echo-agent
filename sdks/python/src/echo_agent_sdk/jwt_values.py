from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True, slots=True, repr=False)
class JwtConfig:
    _algorithm: str | None
    _verification_key: str | None = field(repr=False)
    issuer: str | None = None
    audience: str | None = None

    @classmethod
    def hs256(cls, secret: str) -> JwtConfig:
        if not isinstance(secret, str):
            raise TypeError("JWT secret must be text")
        return cls("HS256", secret)

    @classmethod
    def rs256(cls, public_key: str) -> JwtConfig:
        if not isinstance(public_key, str) or not _looks_like_rsa_public_key(
            public_key
        ):
            raise ValueError("invalid RSA public key")
        return cls("RS256", public_key)

    @classmethod
    def disabled(cls) -> JwtConfig:
        return cls(None, None)

    def with_issuer(self, issuer: str) -> JwtConfig:
        if not isinstance(issuer, str):
            raise TypeError("JWT issuer must be text")
        return JwtConfig(self._algorithm, self._verification_key, issuer, self.audience)

    def with_audience(self, audience: str) -> JwtConfig:
        if not isinstance(audience, str):
            raise TypeError("JWT audience must be text")
        return JwtConfig(self._algorithm, self._verification_key, self.issuer, audience)

    def is_enabled(self) -> bool:
        return self._algorithm is not None

    def __repr__(self) -> str:
        algorithm = self._algorithm or "none"
        return (
            "JwtConfig("
            f"enabled={self.is_enabled()}, algorithm={algorithm!r}, issuer={self.issuer!r}, "
            "audience="
            f"{self.audience!r}, verification_key='[redacted]')"
        )


@dataclass(frozen=True, slots=True)
class JwtClaims:
    iss: str | None = None
    sub: str | None = None
    aud: str | None = None
    exp: int | None = None
    nbf: int | None = None
    iat: int | None = None
    jti: str | None = None
    extra: dict[str, Any] = field(default_factory=dict)

    def subject(self) -> str | None:
        return self.sub


def _looks_like_rsa_public_key(value: str) -> bool:
    trimmed = value.strip()
    return (
        trimmed.startswith("-----BEGIN PUBLIC KEY-----")
        and trimmed.endswith("-----END PUBLIC KEY-----")
    ) or (
        trimmed.startswith("-----BEGIN RSA PUBLIC KEY-----")
        and trimmed.endswith("-----END RSA PUBLIC KEY-----")
    )
