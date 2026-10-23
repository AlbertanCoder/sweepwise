# Privacy

**Sweepwise collects nothing.** There is no account, no tracking, no analytics and no crash reporting, and there are
no Sweepwise servers in the app to send anything to. Your files, scans and choices stay on your computer, apart from what the
table below lists.

## What can leave your computer, and only then

| When | Where it goes | What is sent |
| --- | --- | --- |
| Once a day, unless you switch it off in Settings | GitHub (`api.github.com`) | A request for the newest Sweepwise version. Like any web visit, GitHub sees your IP address and the Sweepwise version number. |
| During Scan & Clean and the Security check, and when you approve an app update or install ClamAV | Your computer's own update tools, which go online: winget on Windows, Homebrew and Software Update on a Mac, Flatpak or the package manager on Linux | That tool's own check for which apps and system updates are waiting, or its download. Like any web visit, the tool's servers see your IP address. |
| When Windows Security or ClamAV updates its virus list (Sweepwise can ask it to) | Microsoft or ClamAV's own servers | The scanner's own virus-list download. |
| Only if you add your own AI key | Anthropic (Claude) or OpenAI, directly | File and folder names, sizes and dates, Sweepwise's health and security results, the names of apps and folders you used recently, what you type to it (chat, Organizer requests, your own notes) and, for email, senders and subject lines. Never what is inside a file. Names and subjects that look private are left out, using a list of words. |
| Only if you connect an email account | Your own email provider, directly | Sweepwise signs in to your mailbox to read message headers (sender, subject, date and mailing-list details, plus who you wrote to in the last year; never the message itself) and to move mail you approved to Trash. |
| Only when you press Unsubscribe | The sender's own unsubscribe address | The unsubscribe request the sender put in its email. |
| Only when you press a link (Support, Help, GitHub, the licence, or the guides for getting an AI key or app password) | That website, in your browser: Ko-fi, GitHub, gnu.org, Anthropic, OpenAI, Google, Yahoo or Apple | Whatever any visit to that web page sends. Sweepwise never sees payment details. |

## What stays on your computer

- Sweepwise's history, settings and Undo records live in its own folder. The About & privacy page opens it.
- Your AI key and email app passwords are kept encrypted: on Windows with Windows' own protection for your account;
  on Linux and Mac in a sealed file whose key is kept in your system keychain (GNOME Keyring or KWallet on Linux, the
  login keychain on a Mac), or, on a computer without a keychain, in a key file only your account can read.
- Screen errors are written to a small file in that folder so problems can be found. It is never sent anywhere.

## Your choices

- Switch off the update check in Settings.
- Remove your AI key in Settings, or an email account on the Email page, at any time; Sweepwise deletes its saved copy.
- Before uninstalling, switch off the weekly reminder and auto-clean in Settings. After uninstalling, delete
  Sweepwise's folder to remove its records (on Windows, the uninstaller's "Delete the application data" box does this).

## The help chat on the website (not in the app)

The optional help chat on the Sweepwise website is the one place a Sweepwise server is involved. What you type goes to a small
Sweepwise service running on Cloudflare, which passes it to an AI company (DeepSeek) to write the answer. Emails, user folder
names, phone-like numbers, keys and links are removed first, and Sweepwise does not store your messages: only a daily counter
tied to a scrambled form of your connection, used to cap use, is kept. Cloudflare and DeepSeek handle the messages under
their own policies. If you choose "Send it" when the chat offers to pass a problem to the maker, a short summary with
personal details removed is posted publicly as a GitHub issue. The Sweepwise app never talks to this service.
