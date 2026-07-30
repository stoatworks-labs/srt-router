#!/usr/bin/env python3
"""
Serve a built demo the way GitHub Pages will, for local verification.

`python -m http.server` is not a good enough stand-in: it omits `charset=utf-8`
from Content-Type, so any non-ASCII in the app's JavaScript renders as mojibake
locally while being perfectly fine once published. That wastes time chasing an
encoding bug that doesn't exist. Pages sends the charset; so does this.

Usage:
    serve-demo.py --dir demo/dist --base /flock/ [--port 4285]

--base mounts the site at a subdirectory, matching the project-pages URL.
"""

import argparse
import functools
import http.server
import os
import shutil
import tempfile

CHARSET_TYPES = {
    ".html": "text/html; charset=utf-8",
    ".js": "application/javascript; charset=utf-8",
    ".mjs": "application/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".svg": "image/svg+xml",
}


class PagesHandler(http.server.SimpleHTTPRequestHandler):
    def guess_type(self, path):
        _, ext = os.path.splitext(str(path).lower())
        return CHARSET_TYPES.get(ext) or super().guess_type(path)

    def end_headers(self):
        # Pages doesn't cache aggressively for these, and a stale cache during
        # verification is worse than a slow reload.
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", required=True)
    parser.add_argument("--base", default="/")
    parser.add_argument("--port", type=int, default=4285)
    args = parser.parse_args()

    root = args.dir
    tmp = None
    if args.base not in ("", "/"):
        # Mirror the site under its base path so absolute asset URLs resolve
        # exactly as they will once published.
        tmp = tempfile.mkdtemp(prefix="pages-demo-")
        target = os.path.join(tmp, args.base.strip("/"))
        shutil.copytree(args.dir, target)
        root = tmp

    handler = functools.partial(PagesHandler, directory=root)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler)
    print(f"serving {args.dir} at http://127.0.0.1:{args.port}{args.base}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        if tmp:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
