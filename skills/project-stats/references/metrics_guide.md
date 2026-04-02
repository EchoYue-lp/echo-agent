# Project Health Metrics Guide

## Code Volume

| Metric | Healthy Range | Concern Threshold |
|--------|--------------|-------------------|
| Total LOC | — | >100k (consider splitting) |
| Largest file | <500 lines | >1000 lines |
| Blank line ratio | 15-25% | <10% (too dense) or >40% (padded) |

## Technical Debt Indicators

| Marker | Severity | Action |
|--------|----------|--------|
| `TODO` | Low | Track in backlog |
| `FIXME` | Medium | Schedule for next sprint |
| `HACK` | High | Refactor ASAP |
| `XXX` | Critical | Investigate immediately |

**Debt density**: TODOs per 1000 LOC

| Density | Rating |
|---------|--------|
| <1 | Excellent |
| 1-3 | Normal |
| 3-5 | Needs attention |
| >5 | High debt |

## Dependency Health

- **Direct deps**: ideally <30 for medium projects
- **Dev deps**: no hard limit, but watch for overlap with direct
- **Outdated deps**: check with `npm outdated` / `cargo outdated`
