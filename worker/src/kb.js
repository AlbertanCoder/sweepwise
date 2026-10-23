// The only facts the help chat may use. Everything here is taken from the website, README.md and PRIVACY.md.
export const KB = `
WHAT SWEEPWISE IS
Sweepwise is free PC care for Windows (Linux and Mac are in testing). It clears junk, checks that the computer is protected, speeds it up with safe settings you can undo, and suggests tidy-ups, in plain English. Every feature is free for everyone. No account, no ads, no paid version. It is open source (GPL-3.0). It is made by AlbertanCoder.

THREE PROMISES
1. Nothing personal changes without the person's OK. Temporary files and caches are cleared when they run Check my PC (the first time, only after they have seen how much and pressed Clean now). Everything else waits on the Approve list until they press Approve, and they can untick anything first.
2. Changes they approve can be undone, except uninstalls and app updates. Their own files only move or go to the Recycle Bin, and Sweepwise never empties it. Temporary files and caches, and the old system files Deep Clean clears, are deleted for good.
3. Nothing is collected: no account, no tracking, no analytics, no crash reports. The app has no Sweepwise servers.

WHAT IT DOES
- Check my PC: clears temporary files and caches, runs a quick Windows Security scan and gives a health score with the reasons.
- Approve list: tidies the Desktop and Downloads into folders, finds leftovers from AI tools, programs not opened in six months, startup programs and folders removed programs left behind. In the app: tick what you want and press Approve; there are "Tick all safe items" and "Clear all" buttons.
- Security check: protection status, odd startup items, risky browser extensions, hijacked search or proxy settings, and Microsoft's Malicious Software Removal Tool.
- Speed Boost: fewer programs starting with Windows, lighter settings on slower PCs and up-to-date apps, with a before-and-after report. Before changing the power plan or visual effects, or updating apps, Sweepwise asks Windows for a restore point.
- Deep Clean: Windows' own Disk Cleanup for system files (asks for admin).
- Disk map and Space Finder: see what fills the drive; find duplicates and big forgotten files.
- Email (optional): Gmail, Yahoo and iCloud through an app password. Old bulk mail goes to the email's Trash, with Undo.
- AI helper (optional): the person adds their own Claude or OpenAI key for smarter suggestions. That AI company gets file and folder names, sizes and dates, health and security results, what they type, and for email, senders and subject lines. Never what is inside their files. No AI key is needed for anything else.
- History & Undo puts back moved or binned files, trashed email and changed settings.

GETTING AND UPDATING IT
- Windows 10 and 11. The Microsoft Store version is called Sweepwise PC Care and is signed by Microsoft, so Windows does not warn. Store copies update through the Store.
- A copy downloaded straight from GitHub is not signed with a paid certificate, so Windows may show "Windows protected your PC". They can click More info, then Run anyway. GitHub copies check once a day for a newer version (this can be switched off in Settings) and show a banner; nothing updates without a click.
- Website: https://albertancoder.github.io/sweepwise/  Code: https://github.com/AlbertanCoder/sweepwise

IS IT SAFE
- Safe cleaning only touches temporary files and caches in a fixed list of places. Anything personal waits on the Approve list.
- It does not replace antivirus. On Windows, Windows Security does the virus protection; Sweepwise checks it is switched on and runs its scans.
- No registry cleaners, no driver updaters.

MONEY
- Everything is free. Supporting is optional and unlocks nothing: https://ko-fi.com/albertancoder . It is not a charity, so support is not tax-deductible. If Sweepwise helped, a rating on the Microsoft Store helps others find it.

GETTING HELP
- Report a problem: https://github.com/AlbertanCoder/sweepwise/issues/new  (public page: leave out personal details)
- Questions and ideas: https://github.com/AlbertanCoder/sweepwise/discussions

THIS HELP CHAT
- It is an AI assistant. Messages go to a Sweepwise server on Cloudflare, then to OpenRouter, which passes them to an AI model (DeepSeek) to write the answer. Sweepwise does not store the messages. If the person agrees, a short summary of a problem with personal details removed is posted publicly on GitHub for the maker.
`;
