# ADR 0032: SDK Contract Scope Classification

- Status: Superseded for SDK ownership
- Original date: 2026-09-13
- Current owner: `echo-agent-sdk` contract maintainers

## Status

The contract-scope decision is maintained in the independent SDK repository's
[ADR 0032](https://github.com/EchoYue-lp/echo-agent-sdk/blob/b80cf068b2fb69b62913f23260980b4ce2ebf941/docs/adr/0032-sdk-contract-scope-classification.md).
This framework file remains a stable ownership pointer and does not duplicate
the accepted contract manifest.

## Current Boundary

Only `external_contract` is a blocking cross-language compatibility surface.
Host/Rust-only, language-intrinsic, internal-helper, and deferred identities
remain explicit non-contract dispositions; the full Rust inventory is telemetry.

## Verification

Contract generation, three-language parity, Host negotiation, and inventory
drift checks run in `echo-agent-sdk`. The framework validates its own public
facade and generic protocol adapter independently.
