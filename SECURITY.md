# Security

Sweepwise cleans, moves files and changes settings with the person's OK, and some steps run with admin rights after
the system asks (Deep Clean, Windows restore points, Microsoft's removal tool and, on Linux, Timeshift snapshots and
some installs, updates and removals), so security problems matter a lot here.

## Reporting a problem

Private reporting through GitHub opens when Sweepwise goes public: on the Sweepwise repository, open the **Security**
tab and press **Report a vulnerability**. Until then, reply to the message that invited you. Please don't open a public
issue for a security problem.

Include what you found, how to reproduce it and which version and system you used. You will get a reply as soon as
possible, and credit in the release notes if you would like it.

## Supported versions

Once releases start, only the newest release will get security fixes. Sweepwise checks for new versions once a day
(unless switched off). Its update banner opens the new version's download page; it does not install by itself.

## Design safeguards

- The person's own files only move or go to the Recycle Bin or Trash, never deleted for good (temporary files and
  caches, and the old system files Deep Clean clears, are deleted for good). Each change's Undo record is written the
  moment the change is made, before the next change starts.
- The admin helper for Deep Clean accepts only a fixed list of cleanup steps, and writes its progress only to a file
  named like its own progress files, in a `jobs` folder under a folder whose name contains "Sweepwise", never to a link.
- AI keys and email app passwords are encrypted on the computer and only sent to their own service.
- Links shown in update notices only ever open Sweepwise's own GitHub Releases page.
