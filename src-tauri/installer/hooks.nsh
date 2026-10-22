; Sweepwise installer hooks (bundle > windows > nsis > installerHooks).
; The terms page is skipped in silent (/S) and passive (/P) installs, so only an install that showed it records that the
; person agreed. Sweepwise reads the value at start and then doesn't ask again (engine::terms_ok).
!macro NSIS_HOOK_POSTINSTALL
  ${IfNot} ${Silent}
    ${If} $PassiveMode != 1
      WriteRegStr HKCU "Software\AlbertanCoder\Sweepwise" "TermsAccepted" "1"
    ${EndIf}
  ${EndIf}
!macroend
