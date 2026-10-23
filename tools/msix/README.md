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

Open question: the app checks GitHub once a day for a newer version. The Store only wants updates delivered through the Store, so a Store build may need that check switched off or reworded.
