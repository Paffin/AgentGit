"""A Server-Sent Events parser (WHATWG HTML, "Parsing an event stream")."""

from __future__ import annotations

import codecs
from dataclasses import dataclass


@dataclass(frozen=True)
class SseFrame:
    """One dispatched event."""

    event: str
    """The ``event:`` field (``message`` when absent)."""
    data: str
    id: str
    """The last ``id:`` seen on the connection."""
    retry: int | None = None
    """A ``retry:`` hint in milliseconds, when the server sent one."""


class SseParser:
    """Incremental parser: ``feed`` bytes, get frames. Comment lines (heartbeats) produce no frame.

    Handles ``\\n``, ``\\r\\n`` and ``\\r`` line ends, chunk boundaries anywhere (also inside a multi-byte
    character), a leading BOM, and drops an event that is not terminated by a blank line.
    """

    def __init__(self) -> None:
        self._decoder = codecs.getincrementaldecoder("utf-8")()
        self._buffer = ""
        self._first = True
        self._event = ""
        self._data: list[str] = []
        self._last_id = ""
        self._retry: int | None = None

    def feed(self, chunk: bytes, final: bool = False) -> list[SseFrame]:
        self._buffer += self._decoder.decode(chunk, final)
        if self._first and self._buffer:
            if self._buffer[0] == "\ufeff":
                self._buffer = self._buffer[1:]
            self._first = False
        lines: list[str] = []
        start = 0
        i = 0
        buf = self._buffer
        while i < len(buf):
            c = buf[i]
            if c == "\n":
                lines.append(buf[start:i])
                start = i + 1
            elif c == "\r":
                # A trailing "\r" might be the first half of "\r\n": wait for more input.
                if i + 1 == len(buf) and not final:
                    break
                lines.append(buf[start:i])
                if i + 1 < len(buf) and buf[i + 1] == "\n":
                    i += 1
                start = i + 1
            i += 1
        self._buffer = buf[start:]
        if final and self._buffer:
            lines.append(self._buffer)
            self._buffer = ""
        frames: list[SseFrame] = []
        for line in lines:
            frame = self._line(line)
            if frame is not None:
                frames.append(frame)
        return frames

    def _line(self, line: str) -> SseFrame | None:
        if line == "":
            frame = None
            if self._data:
                frame = SseFrame(self._event or "message", "\n".join(self._data), self._last_id, self._retry)
            self._event = ""
            self._data = []
            self._retry = None
            return frame
        if line.startswith(":"):
            return None
        field, sep, value = line.partition(":")
        if sep and value.startswith(" "):
            value = value[1:]
        if field == "event":
            self._event = value
        elif field == "data":
            self._data.append(value)
        elif field == "id" and "\0" not in value:
            self._last_id = value
        elif field == "retry" and value.isdigit():
            self._retry = int(value)
        return None
