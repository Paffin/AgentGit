"""JSON <-> dataclass conversion for the protocol messages.

Field names are the proto field names (snake_case), except that Python keywords get a trailing
underscore (``from`` -> ``from_``); the dataclass field then carries the wire name in its metadata.
Decoding is tolerant, as the protocol requires: unknown fields are ignored, missing ones take the
dataclass default.
"""

from __future__ import annotations

import base64
import dataclasses
import types
import typing
from collections.abc import Mapping, Sequence
from functools import cache
from typing import Any, TypeVar, Union

T = TypeVar("T")


def wire_name(f: dataclasses.Field[Any]) -> str:
    return str(f.metadata.get("wire", f.name))


def encode(value: Any) -> Any:
    """Converts dataclasses, mappings and sequences into JSON-ready values; ``None`` entries are dropped.

    ``bytes`` become standard base64, as the JSON mapping of ``bytes`` fields requires.
    """
    if dataclasses.is_dataclass(value) and not isinstance(value, type):
        out: dict[str, Any] = {}
        for f in dataclasses.fields(value):
            v = getattr(value, f.name)
            if v is not None:
                out[wire_name(f)] = encode(v)
        return out
    if isinstance(value, Mapping):
        return {str(k): encode(v) for k, v in value.items() if v is not None}
    if isinstance(value, (bytes, bytearray, memoryview)):
        return base64.b64encode(bytes(value)).decode("ascii")
    if isinstance(value, Sequence) and not isinstance(value, str):
        return [encode(v) for v in value]
    return value


@cache
def _hints(cls: type[Any]) -> dict[str, Any]:
    return typing.get_type_hints(cls)


def decode(tp: Any, value: Any) -> Any:
    """Builds ``tp`` from decoded JSON ``value``."""
    if value is None or tp is Any:
        return value
    origin = typing.get_origin(tp)
    if origin is Union or origin is types.UnionType:
        args = [a for a in typing.get_args(tp) if a is not type(None)]
        return decode(args[0], value) if len(args) == 1 else value
    if origin is list:
        (inner,) = typing.get_args(tp) or (Any,)
        return [decode(inner, v) for v in value] if isinstance(value, list) else value
    if origin is dict:
        kv = typing.get_args(tp)
        inner = kv[1] if len(kv) == 2 else Any
        return {k: decode(inner, v) for k, v in value.items()} if isinstance(value, dict) else value
    if isinstance(tp, type) and dataclasses.is_dataclass(tp):
        if not isinstance(value, dict):
            return value
        hints = _hints(tp)
        kwargs: dict[str, Any] = {}
        for f in dataclasses.fields(tp):
            wire = wire_name(f)
            if wire in value:
                kwargs[f.name] = decode(hints[f.name], value[wire])
        return tp(**kwargs)
    return value


def decode_as(tp: type[T], value: Any) -> T:
    out = decode(tp, value)
    return out  # type: ignore[no-any-return]
