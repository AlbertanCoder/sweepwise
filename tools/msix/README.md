# Microsoft Store package

`build_msix.ps1` packs the Windows release `sweepwise.exe` into an `.msix`. Microsoft signs it after certification, so no paid certificate is needed.

Needs the Windows SDK (already on Shad's PC) and three values from Partner Center, under Product management > Product identity, once the name Sweepwise is reserved:

```
powershell -File tools\msix\build_msix.ps1 -Exe <sweepwise.exe> -IdentityName <Package/Identity/Name> -Publisher <Package/Identity/Publisher> -PublisherDisplay <Publisher display name>
```

Tested 2026-10-23 with placeholder identity: `makeappx pack` succeeds and the package holds the exe, the manifest and three icons. Not yet tested: installing it (needs a self-signed copy) or Store certification.

Restricted capabilities the Store will ask you to explain (paste into the submission notes):
- `runFullTrust`: Sweepwise is a normal desktop program (Tauri and Rust). It cleans files, changes startup entries and Windows settings and runs Windows' own cleanup tools.
- `unvirtualizedResources`: the cleaning has to reach real folders and registry keys, not a private copy of them.

Updates: a Store copy skips the daily GitHub update check (done 2026-10-23; the Store updates it). The current Store policy (10.2.5, version 7.20) requires Store-only updates for games and Xbox products, not ordinary apps, so this is a safe choice rather than a requirement.

Certification note to write plainly: Speed Boost updates your other programs through winget (policy 10.2.3 says a product must not offer to install secondary software; this updates programs the person already has, after they tick them).
