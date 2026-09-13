"""Client construction.

This API declares no security schemes, so there is no credential to resolve.
"""

from __future__ import annotations

from ..client import Client


def build_client(base_url: str) -> Client:
    return Client(base_url)
