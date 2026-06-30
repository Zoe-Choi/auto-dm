from __future__ import annotations

import argparse
import asyncio
import time

from .db import init_db


def run_process_jobs(limit: int) -> int:
    from .worker import process_jobs

    return asyncio.run(process_jobs(limit=limit))


def main() -> None:
    parser = argparse.ArgumentParser(prog="auto-dm")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("init-db", help="Create or migrate the local database")

    once = subparsers.add_parser("process-once", help="Process one batch of queued DM jobs")
    once.add_argument("--limit", type=int, default=50)

    worker = subparsers.add_parser("worker", help="Continuously process queued DM jobs")
    worker.add_argument("--limit", type=int, default=50)
    worker.add_argument("--interval", type=float, default=5.0)

    args = parser.parse_args()
    if args.command == "init-db":
        init_db()
        return
    if args.command == "process-once":
        print(run_process_jobs(args.limit))
        return
    if args.command == "worker":
        init_db()
        while True:
            processed = run_process_jobs(args.limit)
            if processed == 0:
                time.sleep(args.interval)


if __name__ == "__main__":
    main()
