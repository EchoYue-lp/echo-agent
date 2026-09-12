from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class TeamStrategyKind(str, Enum):
    MANAGER_SUBAGENT = "manager_subagent"
    PIPELINE = "pipeline"
    DEBATE = "debate"
    SWARM = "swarm"


@dataclass(frozen=True, slots=True)
class TeamStrategy:
    kind: TeamStrategyKind
    members: tuple[str, ...] = ()
    judge: str | None = None
    debaters: tuple[str, ...] = ()
    reducer: str | None = None

    @classmethod
    def manager(cls) -> TeamStrategy:
        return cls(TeamStrategyKind.MANAGER_SUBAGENT)

    @classmethod
    def pipeline(cls, members: list[str] | tuple[str, ...]) -> TeamStrategy:
        return cls(TeamStrategyKind.PIPELINE, members=tuple(members))

    @classmethod
    def debate(cls, judge: str, debaters: list[str] | tuple[str, ...]) -> TeamStrategy:
        return cls(TeamStrategyKind.DEBATE, judge=judge, debaters=tuple(debaters))

    @classmethod
    def swarm(cls, reducer: str) -> TeamStrategy:
        return cls(TeamStrategyKind.SWARM, reducer=reducer)

    def name(self) -> str:
        return self.kind.value

    def description(self) -> str:
        return {
            TeamStrategyKind.MANAGER_SUBAGENT: "Manager plans typed tasks, Subagents execute them, and the manager synthesizes",
            TeamStrategyKind.PIPELINE: "Subagents execute in sequence",
            TeamStrategyKind.DEBATE: "Debaters propose independently and a judge synthesizes",
            TeamStrategyKind.SWARM: "Subagents inspect independently and a reducer synthesizes",
        }[self.kind]
