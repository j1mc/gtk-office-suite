#!/usr/bin/env python3
"""Keep the pixel scoreboard in one GitHub issue per board.

`pixel_diff.py --scoreboard` ranks the documents a render-lab run compared
by how far our pixels are from LibreOffice's, worst first. On main that
ranking is published here, so there is always one current list to fix
from the top of, and a pull request's run reads it back to show what its
change moved.

  publish  BOARD.md SCORES.json --key KEY --title TITLE [--run-url URL]
           Create or update the board's issue (label `pixel-scoreboard`,
           found by a hidden marker, never by title). The scores ride in
           the body as hidden JSON for `previous`.
  previous --key KEY
           Print the scores the board last published (`{}` if none), the
           file `pixel_diff.py --previous` reads.

Both take [--repo OWNER/REPO] and need `gh` (issues:write for publish).
"""

import argparse
import json
import re
import subprocess
import sys

LABEL = "pixel-scoreboard"
MARK = "<!-- pixel-scoreboard:{} -->"
SCORES = re.compile(r"<!-- pixel-scores\n(.*?)\n-->", re.S)


def gh(*args, input=None):
    r = subprocess.run(["gh", *args], capture_output=True, text=True, input=input)
    if r.returncode != 0:
        raise RuntimeError(f"gh {' '.join(args[:3])}: {r.stderr.strip()}")
    return r.stdout


def find(key, repo):
    """(number, body) of the board's open issue, or None."""
    out = gh("issue", "list", "--label", LABEL, "--state", "open", "--limit", "50",
             "--json", "number,body", *repo)
    for issue in json.loads(out or "[]"):
        if MARK.format(key) in (issue.get("body") or ""):
            return issue["number"], issue["body"]
    return None


def body(board, scores, key, run_url):
    run = f"\n\nMeasured by [this run]({run_url}). " if run_url else "\n\n"
    return (
        f"{MARK.format(key)}\n{board.rstrip()}{run}"
        "Updated on every run on main; a pull request's render comment shows what it moves. "
        "Reproduce locally with `tools/render-lab/pixel_diff.py OUT --scoreboard board.md --overlay` "
        "(red in `pixel-N.png`: ink only LibreOffice has; blue: only ours).\n\n"
        f"<!-- pixel-scores\n{json.dumps(scores, sort_keys=True)}\n-->\n"
    )


def publish(a, repo):
    with open(a.board) as f:
        board = f.read()
    with open(a.scores) as f:
        scores = json.load(f)
    text = body(board, scores, a.key, a.run_url)
    found = find(a.key, repo)
    if found:
        gh("issue", "edit", str(found[0]), "--body-file", "-", *repo, input=text)
        print(f"updated #{found[0]}")
        return
    subprocess.run(["gh", "label", "create", LABEL, "--color", "5319e7", "--force",
                    "--description", "Ranked pixel diffs against LibreOffice", *repo], capture_output=True)
    url = gh("issue", "create", "--title", a.title, "--label", LABEL, "--body-file", "-", *repo, input=text).strip()
    print(f"created {url}")
    # Pinned where it is seen; a repo's pins are limited, so a refusal is fine.
    subprocess.run(["gh", "issue", "pin", url, *repo], capture_output=True)


def previous(a, repo):
    try:
        found = find(a.key, repo)
    except RuntimeError:
        found = None
    m = SCORES.search(found[1]) if found else None
    print(m.group(1) if m else "{}")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("publish")
    p.add_argument("board")
    p.add_argument("scores")
    p.add_argument("--key", required=True)
    p.add_argument("--title", required=True)
    p.add_argument("--run-url")
    p.add_argument("--repo")
    q = sub.add_parser("previous")
    q.add_argument("--key", required=True)
    q.add_argument("--repo")
    a = ap.parse_args(argv)
    repo = ["--repo", a.repo] if a.repo else []
    (publish if a.cmd == "publish" else previous)(a, repo)
    return 0


if __name__ == "__main__":
    sys.exit(main())
