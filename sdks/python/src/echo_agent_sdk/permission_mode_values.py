from __future__ import annotations

from enum import Enum


class PermissionMode(str, Enum):
    DEFAULT = "default"
    PLAN = "plan"
    ACCEPT_EDITS = "auto-edit"
    BYPASS_PERMISSIONS = "full-auto"
    AUTO = "auto"
    BUBBLE = "bubble"
    DONT_ASK = "dont-ask"
    STRICT_CONFIRM = "strict"

    @classmethod
    def parse(cls, value: str) -> PermissionMode:
        if not isinstance(value, str):
            raise TypeError("permission mode must be text")
        aliases = {
            "default": cls.DEFAULT,
            "ask": cls.DEFAULT,
            "plan": cls.PLAN,
            "auto-edit": cls.ACCEPT_EDITS,
            "autoedit": cls.ACCEPT_EDITS,
            "accept-edits": cls.ACCEPT_EDITS,
            "acceptedits": cls.ACCEPT_EDITS,
            "full-auto": cls.BYPASS_PERMISSIONS,
            "fullauto": cls.BYPASS_PERMISSIONS,
            "bypass": cls.BYPASS_PERMISSIONS,
            "bypass-permissions": cls.BYPASS_PERMISSIONS,
            "bypasspermissions": cls.BYPASS_PERMISSIONS,
            "auto": cls.AUTO,
            "bubble": cls.BUBBLE,
            "dont-ask": cls.DONT_ASK,
            "dontask": cls.DONT_ASK,
            "strict": cls.STRICT_CONFIRM,
            "strict-confirm": cls.STRICT_CONFIRM,
            "strict-confirmation": cls.STRICT_CONFIRM,
        }
        try:
            return aliases[value.strip().lower()]
        except KeyError as error:
            raise ValueError(f"invalid permission mode '{value}'") from error

    def allows_write(self) -> bool:
        return self in {self.BYPASS_PERMISSIONS, self.ACCEPT_EDITS}

    def requires_interaction(self) -> bool:
        return self not in {
            self.BYPASS_PERMISSIONS,
            self.AUTO,
            self.DONT_ASK,
            self.ACCEPT_EDITS,
        }

    def uses_classifier(self) -> bool:
        return self is self.AUTO

    def __str__(self) -> str:
        return self.value
