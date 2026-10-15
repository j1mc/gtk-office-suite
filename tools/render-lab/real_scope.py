#!/usr/bin/env python3
"""Does a pull request's change reach an app's rendering? (render-real.yml, render-parity.yml)

The real-document lab used to run on a pull request only when the corpus or
the lab itself changed, so the engines it exists to watch could not trigger
it: DCK-006 (#1574) changed only `decks-core/` and merged a deck that went
from amber to red, found by a later manual dispatch. A hand-written path
filter is how that happened — and how the oracle's filter came to omit
`suite-common-core` (docs/readiness-2026-09/ci-gates.md) — so the scope is
derived here instead:

  - the app's crate plus every workspace crate it depends on, transitively,
    read from the Cargo manifests (`tables` and `decks` reach `letters-core`
    through their core crates, which no one would list by hand);
  - the inputs every app's render shares: the lockfile, the workspace
    manifest (its `[patch]` section changes layout code), the render lab,
    and the calling workflow (`--workflow`).

render-real.yml asks per app (`--app`): every pull request starts it, so each
app's check always reports, and an app this prints `false` for passes in
seconds instead of rendering the same documents as `main`.
render-parity.yml asks once for all apps (no `--app`: a JSON list of the
apps reached) and starts lab jobs only for those; its report jobs always
run and are the verdict. Strict by design: a manifest that cannot be read
raises rather than shrinking the scope, because a scope that silently finds
nothing is a green check that rendered nothing.

Usage:
  git diff --no-renames --name-only HEAD^1 HEAD > changed.txt
  python3 tools/render-lab/real_scope.py --workflow .github/workflows/render-real.yml \\
      --app decks --changed changed.txt                       # true / false
  python3 tools/render-lab/real_scope.py --workflow .github/workflows/render-parity.yml \\
      --changed changed.txt                                   # ["decks"]
  python3 tools/render-lab/real_scope.py --workflow ... --all # every app, not a pull request
"""

import argparse
import json
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
APPS = ("letters", "tables", "decks")
# What every app's rendering depends on besides its own workspace crates and
# the workflow that renders it.
SHARED = ("Cargo.lock", "Cargo.toml", "tools/render-lab/")
# Dependency tables that end up in the app binary. dev-dependencies do not.
LINKED = ("dependencies", "build-dependencies")


def workspace_crates(crate: str, root: Path = REPO_ROOT, seen: set | None = None) -> set:
    """`crate` plus every workspace crate it links, transitively, as directories."""
    seen = set() if seen is None else seen
    if crate in seen:
        return seen
    manifest = root / crate / "Cargo.toml"
    if not manifest.is_file():
        raise FileNotFoundError(f"{crate} is a path dependency but {manifest} does not exist")
    seen.add(crate)
    with open(manifest, "rb") as handle:
        parsed = tomllib.load(handle)
    tables = [parsed.get(table) or {} for table in LINKED]
    for target in (parsed.get("target") or {}).values():
        tables.extend(target.get(table) or {} for table in LINKED)
    for deps in tables:
        for spec in deps.values():
            if isinstance(spec, dict) and "path" in spec:
                dependency = (root / crate / spec["path"]).resolve().relative_to(root.resolve())
                workspace_crates(dependency.as_posix(), root, seen)
    return seen


def scope(app: str, root: Path = REPO_ROOT, workflow: str | None = None) -> list:
    """Path prefixes (directories end in `/`) and files that reach `app`'s rendering
    in `workflow`, the workflow file that renders it."""
    if app not in APPS:
        raise ValueError(f"unknown app {app!r}; expected one of {APPS}")
    own = [workflow] if workflow else []
    return sorted(f"{crate}/" for crate in workspace_crates(app, root)) + list(SHARED) + own


def reaching(app: str, changed: list, root: Path = REPO_ROOT, workflow: str | None = None) -> list:
    """The changed paths that reach `app`'s rendering."""
    prefixes = scope(app, root, workflow)
    return [path for path in changed
            if any(path == p or (p.endswith("/") and path.startswith(p)) for p in prefixes)]


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--workflow", required=True,
                        help="the calling workflow's path; a change to it reaches every app")
    parser.add_argument("--app", choices=APPS,
                        help="print true/false for this app; without it, print the JSON "
                             "list of every app the change reaches")
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--changed",
                        help="file listing the changed paths, one per line ('-' for stdin)")
    source.add_argument("--all", action="store_true",
                        help="not a pull request: every app is in scope")
    args = parser.parse_args(argv)
    apps = [args.app] if args.app else list(APPS)
    if args.all:
        reached = apps
        print("not a pull request: every app is in scope", file=sys.stderr)
    else:
        if args.changed == "-":
            text = sys.stdin.read()
        else:
            text = Path(args.changed).read_text(encoding="utf-8")
        changed = [line.strip() for line in text.splitlines() if line.strip()]
        print(f"{len(changed)} changed path(s)", file=sys.stderr)
        reached = []
        for app in apps:
            hits = reaching(app, changed, workflow=args.workflow)
            print(f"{app} scope: {' '.join(scope(app, workflow=args.workflow))}", file=sys.stderr)
            print(f"{len(hits)} reach {app}:", file=sys.stderr)
            for path in hits:
                print(f"  {path}", file=sys.stderr)
            if hits:
                reached.append(app)
    if args.app:
        print("true" if reached else "false")
    else:
        print(json.dumps(reached, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
