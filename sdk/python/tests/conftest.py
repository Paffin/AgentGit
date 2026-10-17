from __future__ import annotations

from collections.abc import Iterator

import pytest

from tests.harness import RunningServer, start_server


@pytest.fixture(scope="module")
def server() -> Iterator[RunningServer]:
    """A real ag-server for the test module."""
    s = start_server()
    try:
        yield s
    finally:
        s.stop()
