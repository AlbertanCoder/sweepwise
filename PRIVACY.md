# Privacy

**Sweepwise collects nothing.** There is no account, no tracking, no analytics and no crash reporting, and there are
no Sweepwise servers to send anything to. Your files, scans and choices stay on your computer.

## What can leave your computer, and only then

| When | Where it goes | What is sent |
| --- | --- | --- |
| Once a day, unless you switch it off in Settings | GitHub (`api.github.com`) | A request for the newest Sweepwise version. Like any web visit, GitHub sees your IP address and the Sweepwise version number. |
| Only if you add your own AI key | Anthropic (Claude) or OpenAI, directly | File and folder names, sizes and dates, your computer's health numbers and, for email, senders and subject lines. Never what is inside a file. Names and subjects that look private are left out. |
| Only if you connect an email account | Your own email provider, directly | Sweepwise signs in to your mailbox to read message headers (sender, subject, date and mailing-list details, never the message itself) and to move mail you approved to Trash. |
| Only when you press Unsubscribe | The sender's own unsubscribe address | The unsubscribe request the sender put in its email. |
| Only when you press a Support, Help or GitHub link | Ko-fi or GitHub, in your browser | Whatever any visit to that web page sends. Sweepwise never sees payment details. |

## What stays on your computer

- Sweepwise's history, settings and Undo records live in its own folder. The About & privacy page opens it.
- Your AI key and email app passwords are kept encrypted: on Windows with Windows' own protection for your account;
  on Linux in a sealed file whose key is kept in your system keychain (GNOME Keyring or KWallet), or, on a computer
  without a keychain, in a key file only your account can read.
- Screen errors are written to a small file in that folder so problems can be found. It is never sent anywhere.

## Your choices

- Switch off the update check in Settings.
- Remove your AI key in Settings, or an email account on the Email page, at any time; Sweepwise deletes its saved copy.
- Delete Sweepwise's folder to remove every trace after uninstalling.
