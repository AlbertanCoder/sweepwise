# Contributing to Sweepwise

Thank you for helping. Sweepwise runs on people's own computers and touches their files, so every change is reviewed
carefully and has to keep the promises below. Small, focused pull requests are the easiest to review.

## The rules every change keeps

1. **Never delete personal things for good.** Personal files only move, or go to the Recycle Bin (the Trash on Linux
   and Mac). Sweepwise never empties it. Email only goes to the email's own Trash. Temporary files and caches, and the
   old system files Deep Clean clears, are deleted for good.
2. **Nothing personal changes without the person's OK.** Only the fixed list of safe caches is cleared without asking.
   Anything else becomes an Approve list item. File moves start unticked, except AI Organizer steps and AI suggestions
   the person accepted, which start ticked but still wait for Approve.
3. **Changes the person approves can be undone.** Each change writes its Undo record before the next one starts.
   Uninstalling a program (except on a Mac) and updating an app are the exceptions, and the screens say so.
4. **Nothing is collected.** No telemetry, no analytics, no crash reports, no new network calls unless the person
   asks for that feature. See [PRIVACY.md](PRIVACY.md).
5. **The system's own tools only (plus Homebrew on a Mac).** No registry cleaners, no driver updaters, no third-party
   optimisers. On Windows, Microsoft's own tools; on Linux, the system's package manager, systemd and ClamAV; on a Mac,
   macOS's own tools, and Homebrew (when it is installed) for app updates and ClamAV.
6. **Never read inside secret files.** Files named like passwords, keys or IDs are never opened or fingerprinted, and
   names that look private (by a list of words) are never sent to an AI.
7. **Plain English.** Labels describe what happens ("Move to Documents/Finance"), not jargon. New phrases on the
   screens go in `src/i18n/en.json` (the engine writes progress lines and reasons itself); a phrase that differs on
   Linux or Mac gets a `key@linux` or `key@mac` version.

## Getting started

```
npm ci
cd src-tauri
cargo test -- --test-threads=1
```

The engine is in `src-tauri/src/engine/`, one file per feature, nearly all with their own tests. Tests use throwaway
folders from `testutil.rs` and never touch the real computer, except a few that write throwaway keys under
HKEY_CURRENT_USER (Windows) and a few that send a test file to the real Recycle Bin and restore it. Add a test for
every change in behaviour.

Before opening a pull request:

- `cargo test -- --test-threads=1` passes, with no new warnings.
- On Linux, `bash tools/linux_check.sh` passes too if you changed anything Linux uses.
- `python tools/check_routes.py . --email` passes if you changed the screens.

## Ideas and problems

Ask questions, share ideas and report bugs in [Issues](https://github.com/AlbertanCoder/sweepwise/issues). For
security problems, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

By contributing, you agree that your contribution is licensed under the GNU General Public License, version 3 or
later, like the rest of Sweepwise.
