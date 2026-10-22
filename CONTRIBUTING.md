# Contributing to Sweepwise

Thank you for helping. Sweepwise runs on people's own computers and touches their files, so every change is reviewed
carefully and has to keep the promises below. Small, focused pull requests are the easiest to review.

## The rules every change keeps

1. **Never delete anything for good.** Personal files only move, or go to the Recycle Bin (the Trash on Linux and
   Mac). Sweepwise never empties it. Email only goes to the email's own Trash.
2. **Nothing personal changes without the person's OK.** Only the fixed list of safe caches is cleared without asking.
   Anything else becomes an Approve list item, and file moves start unticked.
3. **Everything can be undone.** Each change writes its Undo record before the next one starts. Uninstalling a program
   is the one exception, and the screens say so.
4. **Nothing is collected.** No telemetry, no analytics, no crash reports, no new network calls unless the person
   asks for that feature. See [PRIVACY.md](PRIVACY.md).
5. **The system's own tools only.** No registry cleaners, no driver updaters, no third-party optimisers. On Windows,
   Microsoft's own tools; on Linux, the system's package manager, systemd and ClamAV.
6. **Never read inside private files.** Files that look private are never opened, fingerprinted or sent to an AI.
7. **Plain English.** Labels describe what happens ("Move to Documents/Finance"), not jargon. Every phrase the screens
   show lives in `src/i18n/en.json`; a phrase that differs on Linux gets a `key@linux` version.

## Getting started

```
npm ci
cd src-tauri
cargo test -- --test-threads=1
```

The engine is in `src-tauri/src/engine/`, one file per feature, each with its own tests. Tests use throwaway folders
from `testutil.rs` and never touch the real computer, except a few that send a test file to the real Recycle Bin and
restore it. Add a test for every change in behaviour.

Before opening a pull request:

- `cargo test -- --test-threads=1` passes, with no new warnings.
- On Linux, `bash tools/linux_check.sh` passes too if you changed anything Linux uses.
- `python tools/check_routes.py . --email` passes if you changed the screens.

## Ideas and problems

Ask questions and share ideas in [Discussions](https://github.com/AlbertanCoder/sweepwise/discussions). Report bugs in
[Issues](https://github.com/AlbertanCoder/sweepwise/issues). For security problems, follow [SECURITY.md](SECURITY.md)
instead of opening a public issue.

By contributing, you agree that your contribution is licensed under the GNU General Public License, version 3 or
later, like the rest of Sweepwise.
