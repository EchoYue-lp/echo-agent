# ADR 0013: Learning Examples and Documentation Boundary

- Status: Accepted
- Date: 2026-08-29

## Context

The framework repository had numbered `demo_*.rs` files, deterministic example
contracts, a small facade-consumer package, and a separate Rust-learning crate.
Their split ownership made it unclear which material was a learning path,
which was a public API contract, and which was production framework code.

The framework is reusable and must not absorb EKO application policy or
runtime projections merely because EKO consumes it. At the same time, new
contributors need one stable entry point for lessons and runnable examples.

## Decision

1. Use the non-published `echo-agent-learning` package as the single owner of
   Rust lessons, numbered demos, comprehensive examples, and deterministic
   public-facade contracts.
2. Preserve the existing numbered demo names, order, teaching intent, feature
   requirements, and bundled fixtures. Add multi-capability scenarios beside
   them using the `comprehensive_*.rs` naming convention.
3. Keep contracts without standalone `main` functions under
   `echo-agent-learning/tests/example_contracts/` and run them through one
   executable harness. Keep framework implementation, unit tests, facade
   tests, and doctests in their existing framework locations.
4. Keep reusable framework API and architecture documentation in `docs/`.
   Learning explanations belong in `echo-agent-learning/docs/`; EKO product
   documentation remains in `echo-agent-cli/docs/`.
5. All learning targets depend on the public `echo_agent` facade and must not
   import framework implementation crates or EKO application internals.

## Alternatives

- Keep demos in the framework root package: rejected because production API
  targets and the learning distribution would remain coupled.
- Keep lessons and demos in separate packages: rejected because contributors
  would have two competing learning entry points.
- Move EKO app-core code into the framework: rejected because product policy,
  persistence, UI projections, and local runtime ownership are not reusable
  framework mechanisms.

## Consequences

The workspace has one discoverable learning package and the original demos
remain available for progressive study. Cargo feature forwarding and explicit
example targets keep optional integrations honest. Framework CI can focus on
reusable implementation and facade behavior, while learning CI compiles the
complete example inventory. Changes to examples, fixtures, docs, or feature
requirements must update the learning contracts and website links together.

