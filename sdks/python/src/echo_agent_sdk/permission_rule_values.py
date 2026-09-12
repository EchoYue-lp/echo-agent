from __future__ import annotations

from enum import Enum


class RuleSource(str, Enum):
    DEFAULT = "default"
    LOCAL_SETTINGS = "localSettings"
    PROJECT_SETTINGS = "projectSettings"
    USER_SETTINGS = "userSettings"
    MANAGED = "managed"
    CLI_ARG = "cliArg"
    SESSION = "session"

    @classmethod
    def parse(cls, value: str) -> RuleSource:
        if not isinstance(value, str):
            raise TypeError("rule source must be text")
        aliases = {
            "default": cls.DEFAULT,
            "localSettings": cls.LOCAL_SETTINGS,
            "local_settings": cls.LOCAL_SETTINGS,
            "projectSettings": cls.PROJECT_SETTINGS,
            "project_settings": cls.PROJECT_SETTINGS,
            "userSettings": cls.USER_SETTINGS,
            "user_settings": cls.USER_SETTINGS,
            "manual": cls.USER_SETTINGS,
            "managed": cls.MANAGED,
            "cliArg": cls.CLI_ARG,
            "cli_arg": cls.CLI_ARG,
            "session": cls.SESSION,
        }
        try:
            return aliases[value]
        except KeyError as error:
            raise ValueError(f"unknown permission rule source: {value}") from error

    def __str__(self) -> str:
        return self.value
