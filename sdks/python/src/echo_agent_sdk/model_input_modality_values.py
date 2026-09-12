from __future__ import annotations

from enum import Enum


class ModelInputModality(str, Enum):
    """Content modality accepted by one configured model."""

    TEXT = "text"
    IMAGE = "image"
    AUDIO = "audio"
    VIDEO = "video"

    @classmethod
    def text_only(cls) -> list[ModelInputModality]:
        return [cls.TEXT]

    @classmethod
    def all_supported(cls) -> list[ModelInputModality]:
        return [cls.TEXT, cls.IMAGE, cls.AUDIO, cls.VIDEO]
