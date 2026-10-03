"""Client construction.

This API declares no security schemes, so there is no credential to resolve.
"""

from __future__ import annotations

from ..client import Client, ClientHooks
from .output import capture_response


def build_client(base_url: str) -> Client:
    return Client(
        base_url,
        hooks=ClientHooks(response=[capture_response]),
    )
