# Sweepwise

Free, open-source PC care for Windows, Linux and Mac, funded by the people who use it.

Sweepwise clears out junk, checks that your computer is protected, speeds it up with safe, reversible settings and
suggests tidy-ups, in plain English. Every feature is free for everyone. There are no ads, no paid version and no
account.

> **Status:** in testing. Downloads will appear on the [Releases page](https://github.com/AlbertanCoder/sweepwise/releases)
> when the first version is out.

## Three promises

1. **Nothing personal changes without your OK.** Safe junk (temporary files and caches) is cleared straight away;
   everything else waits on the Approve list until you tick it.
2. **Everything can be undone,** except uninstalling a program. Files only ever go to the Recycle Bin (the Trash on
   Linux), and Sweepwise never empties it.
3. **Nothing is collected.** No account, no tracking, no analytics, no crash reports. See [PRIVACY.md](PRIVACY.md).

## What it does

- **Scan & Clean:** clears temporary files and caches, runs a quick virus scan (Windows Security on Windows, ClamAV on
  Linux) and gives your computer a health score with the reasons behind it. The very first time, it shows exactly
  what it would remove and waits for you to press Clean now.
- **Approve list:** loose files on your Desktop and in Downloads sorted into folders; old leftovers from AI tools
  (scratch folders, editor caches, downloaded models); programs that start by themselves; programs you haven't used in
  six months (on Linux, your Flatpak and Snap apps); folders left behind by removed programs; app updates. Tick what you want, press Approve, and Undo is
  one click away.
- **Security check:** protection status, odd startup items and scheduled tasks, risky browser extensions, browser and
  network hijacks (search engine, proxy, hosts file, forced browser settings) and built-in safety switches. On Windows
  it also offers Microsoft's Malicious Software Removal Tool and the Microsoft Defender Offline scan.
- **Speed Boost:** fewer programs starting with your computer, less junk, lighter settings on slow computers and
  up-to-date apps, with a before-and-after report. Windows makes a restore point first (Linux: a Timeshift snapshot
  when Timeshift is installed).
- **Deep Clean** (asks for admin): Windows' own Disk Cleanup for system files, with optional DISM and SFC repair checks;
  on Linux, downloaded package files, old system logs and old switched-off snap versions.
- **Disk map and Space Finder:** see what fills your drive; find exact duplicates and big forgotten files.
- **Email (optional):** Gmail, Yahoo and iCloud through an app password (Outlook and Hotmail coming soon). Old bulk mail
  goes to your email's Trash, with Undo; unsubscribe from real senders in one click. People you write to, banks,
  government and receipts are never suggested.
- **AI helper (optional, your own key):** add a Claude or OpenAI key for smarter suggestions, the AI Organizer and a
  chat that answers questions about your computer. Only names, sizes and dates are sent, never what is inside your
  files. A built-in guide shows how to get a key.
- **Weekly reminder** and an optional weekly auto-clean (off unless you turn it on).

Windows maintenance uses Microsoft's own tools only: no registry cleaners, no driver updaters, no tricks.

## Supported systems

| System | Status |
| --- | --- |
| Windows 10 and 11 | In testing |
| Linux: Ubuntu, Linux Mint, Debian, Fedora, Arch | In testing (.deb and AppImage) |
| macOS (Apple Silicon; Intel to follow) | In testing (DMG) |

## Support Sweepwise

Sweepwise is kept going by people who chip in. Supporting unlocks nothing: everything is already free.

- One time, any amount: [Ko-fi](https://ko-fi.com/albertancoder) (US$3 buys a coffee; $5 or $10 helps a lot)
- Every month, from US$2: [Ko-fi](https://ko-fi.com/albertancoder) or [GitHub Sponsors](https://github.com/sponsors/AlbertanCoder)

This isn't a charity, so support isn't tax-deductible. You can also help by telling a friend, reporting a problem or
suggesting an idea in [Discussions](https://github.com/AlbertanCoder/sweepwise/discussions).

## Build it yourself

You need [Rust](https://www.rust-lang.org/tools/install), [Node.js](https://nodejs.org/) and Tauri's
[system prerequisites](https://v2.tauri.app/start/prerequisites/).

```
npm ci
npx tauri build --no-bundle
```

Run the tests:

```
cd src-tauri
cargo test -- --test-threads=1
```

The tests make their own throwaway folders; a few send a test file to the real Recycle Bin (or Trash) and restore it
again.

On Linux, install the web-view libraries first, then `tools/linux_check.sh` builds, tests and packages everything:

```
sudo apt-get install -y libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev libxdo-dev nodejs npm
bash tools/linux_check.sh
```

## How it's made

The screens are one HTML page (`src/index.html`, every phrase in `src/i18n/en.json`) inside a
[Tauri](https://tauri.app/) window. The engine is Rust (`src-tauri/src/engine/`, one file per feature, with tests).
Contributions are welcome and reviewed carefully: see [CONTRIBUTING.md](CONTRIBUTING.md). To report a security
problem privately, see [SECURITY.md](SECURITY.md).

## Licence

Sweepwise is free software under the [GNU General Public License, version 3 or later](LICENSE). You can use it, study
it, share it and change it. Anyone who shares a changed version must share their code under the same licence.

Sweepwise comes with absolutely no warranty, to the extent permitted by law. Made by AlbertanCoder.
