# ADR 0028: Source-First Multilanguage SDK Runtime

- Status: Superseded for repository ownership
- Original date: 2026-09-04
- Current owners: `echo-agent` ACP maintainers and `echo-agent-sdk` maintainers

## Status

The runtime decisions in the original ADR remain historical context. Repository
placement and current SDK ownership are superseded by
[ADR 0049](./0049-extract-sdk-repository.md) and the independent SDK
[ADR 0001](https://github.com/EchoYue-lp/echo-agent-sdk/blob/b80cf068b2fb69b62913f23260980b4ce2ebf941/docs/adr/0001-sdk-repository-boundary.md).

## Current Boundary

`echo-agent` owns the generic ACP schema dependency, Agent adapter, standard
projection, framework runtime authorities, and framework conformance tests.
`echo-agent-sdk` owns the extension protocol, source-built Host, accepted
contract artifacts, three language SDKs, SDK documentation, generators, and
SDK-specific CI. The SDK depends on the framework; the framework does not
depend on SDK crates or wire types.

## Compatibility Authority

The complete Rust public inventory is drift telemetry. Only the accepted
external contract is a blocking cross-language compatibility surface, as
defined by the SDK's [ADR 0031](https://github.com/EchoYue-lp/echo-agent-sdk/blob/b80cf068b2fb69b62913f23260980b4ce2ebf941/docs/adr/0031-sdk-identity-governance-scope.md)
and [ADR 0032](https://github.com/EchoYue-lp/echo-agent-sdk/blob/b80cf068b2fb69b62913f23260980b4ce2ebf941/docs/adr/0032-sdk-contract-scope-classification.md).

## Framework Verification

Changes to the generic ACP adapter and framework runtime remain governed by
the framework source, tests, and current ADRs. SDK Host, protocol, contract,
and language verification belongs in the independent SDK repository.
