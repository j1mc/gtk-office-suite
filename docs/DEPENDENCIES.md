# Patched dependencies

This project normally takes dependencies straight from crates.io. Any entry in
the workspace `[patch.crates-io]` section is an exception, and is recorded
here so it does not become permanent by being forgotten: why it exists, what
was tried first, and how to remove it.

There are currently no patched dependencies. The last one, the `oxml-layout`
fontdb feature-unification fork (`tuna-os/rdocx`, #283, #943), was dropped
with rdocx 0.16: upstream `oxml-layout` 0.14 reads any `fontdb::Source` it
does not match explicitly through fontdb itself, so `memmap`'s `SharedFile`
no longer breaks the build.

A patch must be mirrored in `fuzz/Cargo.toml` (a separate workspace the
root's patches do not reach, #883) and pinned by **`rev`, never a branch** — a
moving ref would make the build irreproducible, which is the opposite of what
the release gate is for. `tests/test_patched_dependencies.py` checks that
every patch entry is pinned by `rev` and documented here.
