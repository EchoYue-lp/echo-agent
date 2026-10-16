---
{
  "schema_version": 1,
  "id": "finding.framework-historical-source-observation-retention",
  "kind": "finding",
  "type": "evidence_gap",
  "status": "open",
  "severity": "high",
  "primary_focus": "contract_evidence",
  "focus": [
    "contract_evidence"
  ],
  "boundary_ref": "boundary.workspace-architecture",
  "behavior_refs": [
    "behavior.workspace-composition"
  ],
  "rule_refs": [
    "rule.framework-layer-ownership"
  ],
  "evidence_refs": [
    "evidence.framework-history-current-reobservation"
  ],
  "audit_refs": [
    "audit.framework-history-current-reobservation"
  ],
  "decision_refs": [],
  "repair_evidence_refs": [
    "evidence.framework-history-current-reobservation"
  ],
  "verification_evidence_refs": [
    "evidence.framework-history-current-reobservation"
  ],
  "rereview_audit_refs": [
    "audit.framework-history-current-reobservation"
  ],
  "discovered_at": "7110a27e7b0e9841dcfa0d4cca1cdbc053d96177"
}
---

# Historical source observation retention

## 问题

[Issue189](https://github.com/EchoYue-lp/echo-agent/issues/189) is the unique tracker. The final canonical run found191 source-reference errors in historical observation revisions. Exact recovery found one originalf310 object, now publicly retained; seven original trees remain unavailable with unknown cause. No GC/cleanup attribution or fabricated replacement is claimed.

## 处理记录

The user explicitly chose auditing and handling the latest code during rapid iteration. Accordingly,22 objects receive a bounded current7110 native observation, supported by three independent read-only current reviews and the already executed full05 regressions. Original SHA/875 document links and execution/review bodies remain historical. Current observation is not old-object recovery or replay of old before/after claims.

## 证据

Current7110 source reviews and named existing full05 results are in `evidence.framework-history-current-reobservation`; original failed canonical logs and historical documents remain retained.

## 触发条件与影响

The current-reference repair is subject to canonical verification and metadata review. This Finding remains open for unavailable historical provenance; it does not require continuing object recovery or block the user's authorized current-code delivery once that verification passes. No risk acceptance or public-contract retirement is asserted. Migrated SDK mutation/classifier/inventory obligations remain with the independent SDK accepted-contract lane; new Host/three-language runtime proof is unverified by this F audit.
