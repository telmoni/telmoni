#!/usr/bin/env bash
# The book: the customer docs, which live in their own repository,
# telmoni/docs, checked out beside this one as ../docs. The console builds
# them in rather than linking a site of their own, so the book shares the
# console's header and theme, an image carries the book it was built with, and
# the console agent reads its own /llms-full.txt. This lays the checkout's
# content/ into web/content/docs, which git ignores: where Fumadocs reads it
# (`web/lib/source.ts`) and the tests that hold a page to the code read it
# (`customer_docs_page`).
#
# One rsync with --delete, so a page removed there goes here too, and no
# unchanged file rewritten, so a watch tick that finds nothing new leaves
# `next dev` alone. `make book` runs it once before every target that builds or
# tests the console; `make web-dev` and `make up` run it again with --watch
# beside `next dev`, so an edit in ../docs reaches the dev server within a
# second. CI checks telmoni/docs out and names that checkout.
#
# Usage:   scripts/book.sh [docs checkout] [--watch]      (default ../docs,
#          relative to this repository)
set -euo pipefail

USAGE="usage: book.sh [docs checkout] [--watch]"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DOCS="../docs"
WATCH=""
for arg in "$@"; do
    case "$arg" in
        --watch) WATCH=1 ;;
        -*) echo "$USAGE" >&2; exit 2 ;;
        *) DOCS="$arg" ;;
    esac
done

if ! BOOK="$(cd "$ROOT" && cd "$DOCS" 2>/dev/null && pwd)/content" || [ ! -d "$BOOK" ]; then
    echo "✗ book: no docs at $DOCS; check telmoni/docs out beside this repository:" >&2
    echo "    git clone https://github.com/telmoni/docs.git ../docs" >&2
    exit 2
fi
OUT="$ROOT/web/content/docs"

sync_once() {
    mkdir -p "$OUT"
    rsync -a --delete "$BOOK/" "$OUT/"
}

sync_once
if [ -n "$WATCH" ]; then
    # Ends with whatever started it: in the background of a non-interactive
    # shell, as `make web-dev` and scripts/up.sh start it, Ctrl-C never
    # reaches it.
    while sleep 1 && kill -0 "$PPID" 2>/dev/null; do
        sync_once
    done
fi
