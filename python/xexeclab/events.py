"""Structured JSONL event log.

One JSON object per line. Every record carries:

    ts     : float  -- unix seconds when the event was emitted
    event  : str    -- event name

plus event-specific fields. Documented event names:

    ingest_start     product, max_trades
    ingest_progress  received
    ingest_complete  received, out
"""

from __future__ import annotations

import json
import time
from pathlib import Path


class EventLog:
    def __init__(self, path: str | Path | None = None):
        self.path = Path(path) if path else None
        self._events: list[dict] = []

    def emit(self, event: str, **fields) -> dict:
        record = {"ts": time.time(), "event": event, **fields}
        self._events.append(record)
        if self.path is not None:
            with open(self.path, "a") as f:
                f.write(json.dumps(record) + "\n")
        return record

    @property
    def events(self) -> list[dict]:
        return list(self._events)
