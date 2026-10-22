# Security

Sweepwise cleans, moves files, changes settings with the person's OK and, for Deep Clean, runs with admin rights, so
security problems matter a lot here.

## Reporting a problem

Please report security problems privately through GitHub: on the Sweepwise repository, open the **Security** tab and
press **Report a vulnerability**. Please don't open a public issue for a security problem.

Include what you found, how to reproduce it and which version and system you used. You will get a reply as soon as
possible, and credit in the release notes if you would like it.

## Supported versions

Only the newest release gets security fixes. Sweepwise checks for new versions once a day (unless switched off) and
offers a one-click update.

## Design safeguards

- Files only go to the Recycle Bin or Trash, never deleted for good; Undo records are written before each change.
- The admin helper for Deep Clean accepts only a fixed list of cleanup steps and writes its progress to a file inside
  Sweepwise's own folder.
- AI keys and email app passwords are encrypted on the computer and only sent to their own service.
- Links shown in update notices only ever open Sweepwise's own GitHub Releases page.
