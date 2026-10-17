"""Small constructors for ``FileChange`` messages."""

from __future__ import annotations

from agentgit._generated.models import FileChange
from agentgit.hashing import encode_base64


def put_file(
    path: str, content: bytes | str, *, executable: bool = False, symlink: bool = False
) -> FileChange:
    """A ``FileChange`` that puts a file. Text is UTF-8 encoded; bytes are base64-encoded as the JSON mapping requires.

    For a symlink, ``content`` is the link target.
    """
    data = content.encode("utf-8") if isinstance(content, str) else bytes(content)
    return FileChange(
        path=path,
        op="put",
        content=encode_base64(data),
        executable=executable,
        file_type="symlink" if symlink else "regular",
    )


def delete_file(path: str) -> FileChange:
    """A ``FileChange`` that deletes a file."""
    return FileChange(path=path, op="delete")
