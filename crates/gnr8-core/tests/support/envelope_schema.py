"""Validate saved results against the keyword subset used by the emitted v1 schema."""
import datetime
import json
import re
import sys
from pathlib import Path


def valid(schema, value):
    supported = {
        "$schema", "$id", "title", "type", "required", "properties", "const",
        "enum", "minimum", "pattern", "format", "oneOf",
    }
    assert not set(schema) - supported, f"unsupported validation keyword: {schema}"
    if "const" in schema and value != schema["const"]:
        return False
    if "enum" in schema and value not in schema["enum"]:
        return False
    types = {
        "object": lambda item: isinstance(item, dict),
        "array": lambda item: isinstance(item, list),
        "string": lambda item: isinstance(item, str),
        "integer": lambda item: isinstance(item, int) and not isinstance(item, bool),
    }
    if "type" in schema and not types[schema["type"]](value):
        return False
    if "minimum" in schema and value < schema["minimum"]:
        return False
    if "pattern" in schema and re.search(schema["pattern"], value) is None:
        return False
    if schema.get("format") == "date-time":
        try:
            stamp = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
            if stamp.tzinfo is None:
                return False
        except ValueError:
            return False
    if isinstance(value, dict):
        if any(key not in value for key in schema.get("required", [])):
            return False
        for key, child in schema.get("properties", {}).items():
            if key in value and not valid(child, value[key]):
                return False
    if "oneOf" in schema and sum(valid(child, value) for child in schema["oneOf"]) != 1:
        return False
    return True


def validate_saved(schema_path, directory):
    schema = json.loads(Path(schema_path).read_text())
    results = list(Path(directory).glob("*.json"))
    assert results, "no saved results to validate"
    for path in results:
        value = json.loads(path.read_text())
        assert valid(schema, value), f"invalid saved result: {path}: {value}"
        broken = dict(value)
        broken.pop("response", None)
        assert not valid(schema, broken), "validation did not reject missing metadata"
        broken = dict(value, version=2)
        assert not valid(schema, broken), "validation did not reject an unknown version"
        for kind, required in {"object": "data", "list": "items", "file": "file"}.items():
            if value["kind"] == kind:
                broken = dict(value)
                broken.pop(required, None)
                assert not valid(schema, broken), f"missing {required} was accepted"


if __name__ == "__main__":
    validate_saved(sys.argv[1], sys.argv[2])
