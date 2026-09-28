from __future__ import annotations

from typing import BinaryIO

import zstandard as zstd

ZstdError = zstd.ZstdError


def stream_writer(destination: BinaryIO, *, level: int):
    return zstd.ZstdCompressor(level=level).stream_writer(destination, closefd=False)
