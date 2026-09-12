from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class SkillResourceEntry:
    kind: str
    relative_path: str


@dataclass(frozen=True, slots=True)
class SkillContent:
    name: str
    skill_dir: str
    instructions: str
    allowed_tools: tuple[str, ...] = ()
    resources: tuple[SkillResourceEntry, ...] = ()

    def to_prompt_block(self) -> str:
        block = (
            f'<skill_content name="{self.name}">\n{self.instructions.strip()}\n\n'
            f"Skill directory: {self.skill_dir}\n"
            "Relative paths in this skill are relative to the skill directory."
        )
        if self.allowed_tools:
            block += (
                "\n\n<allowed_tools>\nThis skill declares the following preferred/allowed tools: "
                f"{', '.join(self.allowed_tools)}\nRuntime enforcement currently applies to the built-in skill tools "
                "such as read_skill_resource and run_skill_script.\n</allowed_tools>"
            )
        if self.resources:
            block += "\n\n<skill_resources>"
            for resource in self.resources:
                block += (
                    f'\n  <file kind="{resource.kind}">{resource.relative_path}</file>'
                )
            block += "\n</skill_resources>"
        return f"{block}\n</skill_content>"
