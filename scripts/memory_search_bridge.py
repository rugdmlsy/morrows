#!/usr/bin/env python3
"""Thin Morrows adapter for Zilliz MemSearch.

Morrows owns memory semantics, authorization and authoritative records.
MemSearch owns only a rebuildable hybrid-search shadow index.
"""

from __future__ import annotations

import argparse
import asyncio
import json
from importlib.metadata import version
from pathlib import Path

MODEL = "gpahal/bge-m3-onnx-int8"
PROVIDER = "onnx"


async def index_search(args: argparse.Namespace) -> dict:
    from memsearch.core import MemSearch

    root = Path(args.root).expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)
    ms = MemSearch(
        [root],
        embedding_provider=PROVIDER,
        embedding_model=MODEL,
        milvus_uri=str(Path(args.milvus_uri).expanduser().resolve()),
        collection=args.collection,
        reranker_model="",
        description="Morrows rebuildable project-memory search index",
    )
    try:
        report = await ms.index_with_report(force=False)
        if report.failed_files:
            raise RuntimeError(
                "MemSearch failed to index projection files: "
                + "; ".join(f"{item.path}: {item.error}" for item in report.failed_files)
            )
        results = await ms.search(
            args.query,
            top_k=args.top_k,
            source_prefix=root,
        )
        return {
            "engine": "memsearch",
            "version": version("memsearch"),
            "provider": PROVIDER,
            "model": MODEL,
            "indexed_chunks": report.indexed_chunks,
            "results": results,
        }
    finally:
        ms.close()


async def doctor(args: argparse.Namespace) -> dict:
    from memsearch.core import MemSearch

    work = Path(args.work_dir).expanduser().resolve()
    work.mkdir(parents=True, exist_ok=True)
    projection = work / "projection"
    projection.mkdir(parents=True, exist_ok=True)
    sentinel = "MORROWS-MEMSEARCH-DOCTOR-ALPHA-17"
    (projection / "doctor.md").write_text(
        f"# Morrows memory search doctor\n\n{sentinel}\n",
        encoding="utf-8",
    )
    db = Path(args.milvus_uri).expanduser().resolve()
    ms = MemSearch(
        [projection],
        embedding_provider=PROVIDER,
        embedding_model=MODEL,
        milvus_uri=str(db),
        collection="morrows_doctor",
        reranker_model="",
        description="Morrows deployment doctor",
    )
    try:
        report = await ms.index_with_report(force=True)
        if report.failed_files:
            raise RuntimeError(str(report.failed_files))
        results = await ms.search("ALPHA-17 Morrows doctor", top_k=3, source_prefix=projection)
        if not any(sentinel in str(item.get("content", "")) for item in results):
            raise RuntimeError("MemSearch doctor could not retrieve its sentinel")
        return {
            "ok": True,
            "engine": "memsearch",
            "version": version("memsearch"),
            "provider": PROVIDER,
            "model": MODEL,
            "result_count": len(results),
        }
    finally:
        ms.close()


async def serve(args: argparse.Namespace) -> None:
    from memsearch.core import MemSearch

    root = Path(args.root).expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True)
    ms = MemSearch(
        [root],
        embedding_provider=PROVIDER,
        embedding_model=MODEL,
        milvus_uri=str(Path(args.milvus_uri).expanduser().resolve()),
        collection=args.collection,
        reranker_model="",
        description="Morrows rebuildable project-memory search index",
    )
    try:
        while True:
            line = await asyncio.to_thread(input)
            if not line:
                continue
            try:
                request = json.loads(line)
                operation = request.get("op")
                if operation == "ping":
                    response = {
                        "ok": True,
                        "engine": "memsearch",
                        "version": version("memsearch"),
                        "provider": PROVIDER,
                        "model": MODEL,
                    }
                elif operation == "search":
                    project = str(request["project_id"])
                    project_root = (root / project).resolve()
                    if project_root.parent != root:
                        raise ValueError("invalid project_id")
                    report = await ms.index_with_report(force=False)
                    if report.failed_files:
                        raise RuntimeError(
                            "MemSearch failed to index projection files: "
                            + "; ".join(f"{item.path}: {item.error}" for item in report.failed_files)
                        )
                    results = await ms.search(
                        str(request["query"]),
                        top_k=int(request["top_k"]),
                        source_prefix=project_root,
                    )
                    response = {
                        "ok": True,
                        "engine": "memsearch",
                        "version": version("memsearch"),
                        "provider": PROVIDER,
                        "model": MODEL,
                        "indexed_chunks": report.indexed_chunks,
                        "results": results,
                    }
                else:
                    raise ValueError(f"unknown operation: {operation!r}")
            except Exception as exc:
                response = {"ok": False, "error": f"{type(exc).__name__}: {exc}"}
            print(json.dumps(response, ensure_ascii=False), flush=True)
    except EOFError:
        return
    finally:
        ms.close()


def parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="command", required=True)

    server = sub.add_parser("serve")
    server.add_argument("--root", required=True)
    server.add_argument("--milvus-uri", required=True)
    server.add_argument("--collection", required=True)

    search = sub.add_parser("index-search")
    search.add_argument("--root", required=True)
    search.add_argument("--milvus-uri", required=True)
    search.add_argument("--collection", required=True)
    search.add_argument("--query", required=True)
    search.add_argument("--top-k", required=True, type=int)

    check = sub.add_parser("doctor")
    check.add_argument("--work-dir", required=True)
    check.add_argument("--milvus-uri", required=True)
    return p


def main() -> None:
    args = parser().parse_args()
    if args.command == "serve":
        asyncio.run(serve(args))
        return
    if args.command == "index-search":
        result = asyncio.run(index_search(args))
    else:
        result = asyncio.run(doctor(args))
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
