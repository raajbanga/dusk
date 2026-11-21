; Remove the start-with-Windows entry and the app's settings key.
!macro NSIS_HOOK_POSTUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Dusk"
  DeleteRegKey HKCU "Software\Dusk"
!macroend
