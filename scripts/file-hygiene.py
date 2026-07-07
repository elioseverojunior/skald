#!/usr/bin/env python3
"""Portable file-hygiene checks — the lefthook replacement for the
`pre-commit/pre-commit-hooks` repo (v6.0.0).

pre-commit-hooks ships these checks as small Python programs; this script
reimplements the ones the project used in a single stdlib-only file that
lefthook drives with `{staged_files}`. One file, zero dependencies, same intent.

Checks (validators — always block on violation):
    check-added-large-files   (--maxkb=1000)
    check-merge-conflict
    check-case-conflict
    check-symlinks
    check-executables-have-shebangs
    check-toml
    check-json
    forbid-new-submodules
Fixers (rewrite the file in place when --fix is given; otherwise reported):
    trailing-whitespace       (markdown hard-breaks preserved on *.md)
    end-of-file-fixer
    mixed-line-ending         (--fix=lf)

Intentionally NOT reimplemented — the project already handles these elsewhere:
    check-yaml          -> yamllint job (stricter, multi-document aware)
    detect-private-key  -> gitleaks job (broader secret scanning)
    detect-secrets      -> gitleaks job

Usage:
    file-hygiene.py [--fix] [paths...]   # paths come from lefthook {staged_files}
With no paths, there is nothing staged to check, so it exits 0.
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path

MAX_KB = 1000
# Vendored YAML conformance fixtures must never be reformatted (matches the
# .yamllint.yml ignore). Keep this in sync with that ignore list.
IGNORE_PREFIXES = ("crates/skald-yaml-test-suite/",)

CONFLICT_MARKERS = (b"<<<<<<< ", b"======= ", b">>>>>>> ", b"|||||||")
# `=======` with nothing after it is also a conflict marker line.
CONFLICT_EXACT = (b"=======", b"<<<<<<<", b">>>>>>>")


class Report:
    """Accumulates violations and (in --fix mode) the set of rewritten files."""

    def __init__(self) -> None:
        self.errors: list[str] = []
        self.fixed: set[str] = set()

    def err(self, path: str, msg: str) -> None:
        self.errors.append(f"  {path}: {msg}")

    def fix(self, path: str) -> None:
        self.fixed.add(path)


def is_binary(data: bytes) -> bool:
    return b"\x00" in data[:8192]


def ignored(rel: str) -> bool:
    return any(rel.startswith(p) for p in IGNORE_PREFIXES)


# --------------------------------------------------------------------------- #
# Validators
# --------------------------------------------------------------------------- #
def check_large_file(path: Path, rel: str, rep: Report) -> None:
    kb = path.stat().st_size / 1024
    if kb > MAX_KB:
        rep.err(rel, f"file is {kb:.0f} KB, exceeds the {MAX_KB} KB limit")


def check_merge_conflict(rel: str, data: bytes, rep: Report) -> None:
    for n, line in enumerate(data.splitlines(), 1):
        if line.startswith(CONFLICT_MARKERS) or line.rstrip() in CONFLICT_EXACT:
            rep.err(rel, f"merge-conflict marker on line {n}")
            return


def check_toml(path: Path, rel: str, rep: Report) -> None:
    try:
        with path.open("rb") as fh:
            tomllib.load(fh)
    except tomllib.TOMLDecodeError as exc:
        rep.err(rel, f"invalid TOML: {exc}")


def check_json(path: Path, rel: str, rep: Report) -> None:
    try:
        json.loads(path.read_bytes())
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        rep.err(rel, f"invalid JSON: {exc}")


def check_symlink(path: Path, rel: str, rep: Report) -> None:
    if path.is_symlink() and not path.exists():
        rep.err(rel, "broken symlink (points to a missing target)")


def check_executable_shebang(path: Path, rel: str, data: bytes, rep: Report) -> None:
    # Only meaningful for non-binary files with the user-executable bit set.
    if path.is_symlink() or is_binary(data):
        return
    if path.stat().st_mode & 0o100 and not data.startswith(b"#!"):
        rep.err(rel, "executable file is missing a shebang (#!) — or drop the +x bit")


def check_case_conflict(paths: list[str], rep: Report) -> None:
    """Flag paths that collide when compared case-insensitively (breaks on
    case-insensitive filesystems like default macOS/Windows)."""
    seen: dict[str, str] = {}
    for rel in paths:
        low = rel.lower()
        if low in seen and seen[low] != rel:
            rep.err(rel, f"case-only collision with {seen[low]!r}")
        else:
            seen[low] = rel


def _declared_submodule_paths() -> set[str]:
    """Submodule paths registered in .gitmodules (the intentional submodules)."""
    import subprocess  # local import: only the git-aware check needs it

    try:
        out = subprocess.run(
            ["git", "config", "--file", ".gitmodules",
             "--get-regexp", r"^submodule\..*\.path$"],
            capture_output=True, text=True, check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return set()  # no .gitmodules (or no entries) / git unavailable
    # each line: "submodule.<name>.path <path>"
    return {parts[1] for line in out.splitlines()
            if len(parts := line.split(None, 1)) == 2}


def check_new_submodule(rep: Report) -> None:
    """Block newly-added gitlinks (submodules) UNLESS declared in .gitmodules.

    A submodule registered in .gitmodules is intentional — the project vendors
    the upstream yaml-test-suite as `crates/skald-yaml-test-suite/data` this way.
    An *undeclared* gitlink is almost always an accidental `git add` of a nested
    clone, which is exactly what this guard still forbids.
    """
    import subprocess  # local import: only the git-aware check needs it

    try:
        out = subprocess.run(
            ["git", "diff", "--cached", "--raw", "--diff-filter=A"],
            capture_output=True, text=True, check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return  # not a git context / git unavailable — skip silently
    declared = _declared_submodule_paths()
    for line in out.splitlines():
        # raw format: :<oldmode> <newmode> <oldsha> <newsha> <status>\t<path>
        if line.startswith(":") and line.split()[1] == "160000":
            path = line.split("\t", 1)[-1]
            if path not in declared:
                rep.err(path, "new git submodule is forbidden "
                              "(not declared in .gitmodules)")


# --------------------------------------------------------------------------- #
# Fixers — return the corrected bytes (or None if already clean).
# --------------------------------------------------------------------------- #
def fix_line_endings(data: bytes) -> bytes | None:
    """mixed-line-ending --fix=lf: normalize CRLF/CR to LF."""
    new = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")
    return new if new != data else None


def fix_trailing_whitespace(data: bytes, is_md: bool) -> bytes | None:
    """trailing-whitespace; on markdown, preserve a 2-space hard line break."""
    out_lines = []
    for line in data.split(b"\n"):
        stripped = line.rstrip(b" \t")
        if is_md and line.endswith(b"  ") and line.strip():
            stripped = stripped + b"  "  # keep markdown hard break
        out_lines.append(stripped)
    new = b"\n".join(out_lines)
    return new if new != data else None


def fix_end_of_file(data: bytes) -> bytes | None:
    """end-of-file-fixer: exactly one trailing newline (none for empty files)."""
    if not data:
        return None
    new = data.rstrip(b"\n") + b"\n"
    return new if new != data else None


def run_fixers(path: Path, rel: str, data: bytes, do_fix: bool, rep: Report) -> None:
    is_md = rel.endswith(".md")
    new = data
    for fixer in (
        lambda d: fix_line_endings(d),
        lambda d: fix_trailing_whitespace(d, is_md),
        lambda d: fix_end_of_file(d),
    ):
        result = fixer(new)
        if result is not None:
            new = result
    if new == data:
        return
    if do_fix:
        path.write_bytes(new)
        rep.fix(rel)
    else:
        rep.err(rel, "whitespace/line-ending/EOF issue — run `mise run hygiene-fix`")


# --------------------------------------------------------------------------- #
def main(argv: list[str]) -> int:
    args = argv[1:]
    do_fix = "--fix" in args
    paths = [a for a in args if a != "--fix"]
    if not paths:
        return 0

    rep = Report()
    check_case_conflict(paths, rep)
    check_new_submodule(rep)

    for rel in paths:
        if ignored(rel):
            continue
        path = Path(rel)
        if not path.exists() or not path.is_file():
            continue  # deleted/renamed-away staged path

        check_symlink(path, rel, rep)
        check_large_file(path, rel, rep)
        try:
            data = path.read_bytes()
        except OSError:
            continue

        check_executable_shebang(path, rel, data, rep)
        if path.suffix == ".toml":
            check_toml(path, rel, rep)
        if path.suffix == ".json":
            check_json(path, rel, rep)

        if is_binary(data):
            continue  # text-content checks/fixers don't apply to binaries
        check_merge_conflict(rel, data, rep)
        run_fixers(path, rel, data, do_fix, rep)

    if rep.fixed:
        print(f"file-hygiene: fixed {len(rep.fixed)} file(s):", file=sys.stderr)
        for f in sorted(rep.fixed):
            print(f"  {f}", file=sys.stderr)
    if rep.errors:
        print("file-hygiene: violations:", file=sys.stderr)
        print("\n".join(rep.errors), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
