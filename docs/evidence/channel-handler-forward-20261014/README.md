# Channel handler forward evidence archive

These logs are copied byte-for-byte from the original local CommandExecution
aggregated_output records. command-receipts.json preserves command, exit code,
time, record ordinal and log hash. No commands were rerun to fabricate these
historical results. First compilation and lint failures remain present.

source-freeze.json and final-sources retain the 80353501 candidate at the end
of the first focused validation: 22 new tests, 79 full Channel tests, 12 native
Agent tests, 2 doctests, warning and panic lint groups passed. Earlier command
attempts precede the narrow source fixes and must not be reported as final-source
validation. previous-sources SHA matches the prior review's ff665397 manager
and a44e937d Session anchors exactly; review-increment diffs are complete.

The later reviewer found retire/open's ready=false -> cancel signalling window.
current-source-freeze.json binds the repaired original admission mutex scope and
actual-main merge. The original logs are historical evidence, **not** validation
of that later source. The barrier counterexample concerns lock ownership at the
signal boundary; it does not claim a real handler message was already misadmitted.
New focused tests/lint and final full gates remain pending for that source.

Formal API docs and ADR 0085 are synchronized. The module's executable example
compiled in the two doctests. Existing standalone Channel examples use retained
APIs and will be covered by the final workspace all-target gates. Website
synchronization is checked separately below; language SDK evidence remains SDK
owned and does not expand Rust inventory into accepted identities.

Website inspection found echo-website/public/llms-full.txt includes the Channel
reference. Synchronization is applicable and remains pending regeneration from
the published framework documentation plus website checks; it is not marked
inapplicable. No website files were changed by this framework subagent.
