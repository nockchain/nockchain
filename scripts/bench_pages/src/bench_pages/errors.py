from __future__ import annotations


class PublishError(Exception):
    """Base error for the sweep publisher."""


class ValidationError(PublishError):
    """Raised when a sweep artifact tree is incomplete or inconsistent."""


class ExternalCommandError(PublishError):
    """Raised when an external command exits unsuccessfully."""

