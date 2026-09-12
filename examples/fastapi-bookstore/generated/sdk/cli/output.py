"""Rendering a result on stdout: JSON for a document, raw bytes for a file."""

from __future__ import annotations

import json
import sys
from typing import Any

from pydantic import BaseModel


def _jsonable(value: Any) -> Any:
    if isinstance(value, BaseModel):
        return value.model_dump(mode="json")
    if isinstance(value, list):
        return [_jsonable(item) for item in value]
    if isinstance(value, dict):
        return {key: _jsonable(item) for key, item in value.items()}
    return value


def print_result(result: Any) -> None:
    if isinstance(result, (bytes, bytearray)):
        sys.stdout.buffer.write(result)
        return
    json.dump(_jsonable(result), sys.stdout, indent=2)
    sys.stdout.write("\n")
