"""The generated modules are in sync with docs/protocol/openapi.json and sdk/api-policy.json."""

from __future__ import annotations

import dataclasses
import json
import subprocess
import sys
import typing
from pathlib import Path
from typing import Any

import pytest

import agentgit
from agentgit import AgentGitClient, AsyncAgentGitClient, models
from agentgit._codec import decode_as, encode
from tests.components import not_applicable
from tests.harness import REPO_ROOT

SPEC = json.loads((REPO_ROOT / "docs/protocol/openapi.json").read_text())
POLICY = json.loads((REPO_ROOT / "sdk/api-policy.json").read_text())
PYTHON_ROOT = Path(__file__).resolve().parents[1]


def test_regenerating_produces_no_diff() -> None:
    result = subprocess.run(
        [sys.executable, str(PYTHON_ROOT / "scripts/generate.py"), "--check"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def is_string_enum(schema: dict[str, Any]) -> bool:
    return schema.get("type") == "string" and "enum" in schema


def test_every_schema_of_the_openapi_document_has_a_dataclass() -> None:
    for name, schema in SPEC["components"]["schemas"].items():
        model = getattr(models, name, None)
        assert model is not None, f"no model for schema {name}"
        if not is_string_enum(schema):
            assert dataclasses.is_dataclass(model)
    assert "PutObjectResult" in models.__all__


def test_a_closed_set_of_strings_is_a_literal_type_not_a_class() -> None:
    enums = {n: s for n, s in SPEC["components"]["schemas"].items() if is_string_enum(s)}
    assert "LoginToken" in enums  # the one the sign-in configuration uses
    for name, schema in enums.items():
        alias = getattr(models, name)
        assert typing.get_origin(alias) is typing.Literal, name
        assert list(typing.get_args(alias)) == schema["enum"], name
    # the message that carries it declares the field with the alias, and decodes the plain string
    hints = typing.get_type_hints(models.AuthProvider)
    assert models.LoginToken in typing.get_args(hints["token"])
    provider = decode_as(models.AuthProvider, {"name": "idp", "issuer": "https://idp", "token": "id_token"})
    assert provider.token == "id_token"
    assert provider.authorization_endpoint is None


def test_nullable_strings_are_typed_as_strings() -> None:
    hints = typing.get_type_hints(models.ConfigureRepoRequest)
    assert set(typing.get_args(hints["description"])) == {str, type(None)}
    hints = typing.get_type_hints(models.AuthProvider)
    for field in ("authorization_endpoint", "token_endpoint", "end_session_endpoint"):
        assert set(typing.get_args(hints[field])) == {str, type(None)}, field


def test_model_fields_are_the_wire_fields() -> None:
    for name, schema in SPEC["components"]["schemas"].items():
        if is_string_enum(schema):
            continue
        cls = getattr(models, name)
        wire = {str(f.metadata.get("wire", f.name)) for f in dataclasses.fields(cls)}
        assert wire == set(schema.get("properties", {})), name


def test_the_operation_table_matches_the_openapi_document() -> None:
    from agentgit import OPERATIONS

    seen = set()
    for route, item in SPEC["paths"].items():
        for verb, op in item.items():
            info = OPERATIONS[op["operationId"]]
            assert (info.http, info.route) == (verb.upper(), route)
            seen.add(info.id)
    assert seen == set(OPERATIONS)


def test_the_policy_file_names_real_operations_and_fields() -> None:
    ids = {o["operationId"] for item in SPEC["paths"].values() for o in item.values()}
    r = POLICY["retry"]
    for group in (
        r["readPosts"],
        r["keyed"],
        r["natural"],
        list(r["naturalWhenFieldPresent"]),
        [k for k in POLICY["bodies"] if not k.startswith("$")],
    ):
        assert set(group) <= ids
    from agentgit import OPERATIONS

    for op_id in r["keyed"]:
        assert OPERATIONS[op_id].idempotency_key  # a keyed operation must take a key
    for field in POLICY["alwaysPresent"]["fields"]:
        schema, prop = field.split(".")
        hints = typing.get_type_hints(getattr(models, schema))
        assert "None" not in str(hints[prop]), f"{field} should be typed as present"


def test_the_paginate_overloads_cover_exactly_the_paginated_operations() -> None:
    if sys.version_info < (3, 11):
        not_applicable(
            "python-version",
            "test_the_paginate_overloads_cover_exactly_the_paginated_operations",
            "typing.get_overloads needs Python 3.11",
        )
    get_overloads = typing.cast("Any", getattr(typing, "get_overloads", None))
    for client in (AgentGitClient, AsyncAgentGitClient):
        literals: set[str] = set()
        for fn in get_overloads(client.paginate):
            hint = typing.get_type_hints(fn)["op_id"]
            literals.update(typing.get_args(hint))
        assert literals == set(agentgit.PAGINATED), client.__name__


def test_wire_conversion_round_trips_nested_messages_and_keywords() -> None:
    req = models.DiffRequest(repo="a/b", from_="x", to="y", paths=["p"])
    assert encode(req)["from"] == "x"
    assert "from_" not in encode(req)
    ws = decode_as(
        models.ModifyWorkspaceRequest,
        {
            "repo": "a/b",
            "name": "w",
            "changes": [{"path": "p", "op": "put", "content": "eA==", "future_field": 1}],
        },
    )
    assert ws.changes[0].content == "eA=="
    # unknown fields are ignored, missing ones take the zero value
    assert ws.changes[0].executable is False
    assert encode(models.FileChange(path="p", content=b"x")) == {  # type: ignore[arg-type]
        "path": "p", "op": "", "blob_id": "", "content": "eA==", "executable": False, "file_type": ""
    }  # fmt: skip
    outcome = decode_as(models.IntegrationResult, {"operation_id": "op_1"})
    assert outcome.outcome.status == ""  # present even when the server omits it


@pytest.mark.parametrize("cls", [models.Principal, models.NegotiateResponse, models.Repo])
def test_always_present_messages_are_never_none(cls: type) -> None:
    assert decode_as(cls, {}) is not None
    for f in dataclasses.fields(cls):
        if f.name in ("scope", "limits", "quota"):
            assert getattr(decode_as(cls, {}), f.name) is not None
