; Per-user shell extension. Keep the previous provider for uninstall/upgrade.
!define TMV_THUMB_CLSID "{926e1d8f-eb25-48d9-a251-9d27bf165d8a}"
!define TMV_THUMB_SLOT "{e357fccd-a995-4576-b01f-234630154e96}"
!define TMV_PLUGIN_RECORD "Software\com.leehow.tifmrcview\PluginFormats"
!define TMV_PLUGIN_PROGID "MRC Image"
!define TMV_THUMB_BACKUP "Software\com.leehow.tifmrcview\ThumbnailBackup"

!macro TMV_REGISTER_EXT EXT
  ReadRegStr $0 HKCU "Software\Classes\.${EXT}\ShellEx\${TMV_THUMB_SLOT}" ""
  ${If} $0 != "${TMV_THUMB_CLSID}"
    WriteRegStr HKCU "${TMV_THUMB_BACKUP}" "${EXT}" "$0"
  ${EndIf}
  WriteRegStr HKCU "Software\Classes\.${EXT}\ShellEx\${TMV_THUMB_SLOT}" "" "${TMV_THUMB_CLSID}"
!macroend

!macro TMV_UNREGISTER_EXT EXT
  ReadRegStr $0 HKCU "Software\Classes\.${EXT}\ShellEx\${TMV_THUMB_SLOT}" ""
  ${If} $0 == "${TMV_THUMB_CLSID}"
    ReadRegStr $1 HKCU "${TMV_THUMB_BACKUP}" "${EXT}"
    ${If} $1 == ""
      DeleteRegKey HKCU "Software\Classes\.${EXT}\ShellEx\${TMV_THUMB_SLOT}"
    ${Else}
      WriteRegStr HKCU "Software\Classes\.${EXT}\ShellEx\${TMV_THUMB_SLOT}" "" "$1"
    ${EndIf}
  ${EndIf}
  DeleteRegValue HKCU "${TMV_THUMB_BACKUP}" "${EXT}"
!macroend

!macro TMV_EACH_EXT ACTION
  !insertmacro ${ACTION} tif
  !insertmacro ${ACTION} tiff
  !insertmacro ${ACTION} mrc
  !insertmacro ${ACTION} mrcs
  !insertmacro ${ACTION} rec
  !insertmacro ${ACTION} st
  !insertmacro ${ACTION} map
  !insertmacro ${ACTION} ali
  !insertmacro ${ACTION} preali
  !insertmacro ${ACTION} czi
  !insertmacro ${ACTION} lsm
  !insertmacro ${ACTION} lif
  !insertmacro ${ACTION} lof
  !insertmacro ${ACTION} nd2
  !insertmacro ${ACTION} oib
  !insertmacro ${ACTION} oir
  !insertmacro ${ACTION} ims
!macroend

; Runtime journals only entries it introduced. Do not remove whole extension keys.
!macro TMV_UNREGISTER_PLUGINS
  Push $2
  Push $3
  Push $4
  ${Do}
    EnumRegKey $2 HKCU "${TMV_PLUGIN_RECORD}" 0
    ${If} $2 == ""
      ${ExitDo}
    ${EndIf}
    ReadRegStr $3 HKCU "${TMV_PLUGIN_RECORD}\$2" "OpenWith"
    ${If} $3 == "1"
      DeleteRegValue HKCU "Software\Classes\.$2\OpenWithProgids" "${TMV_PLUGIN_PROGID}"
    ${EndIf}
    ReadRegStr $3 HKCU "${TMV_PLUGIN_RECORD}\$2" "Thumbnail"
    ${If} $3 == "1"
      ReadRegStr $3 HKCU "Software\Classes\.$2\ShellEx\${TMV_THUMB_SLOT}" ""
      ${If} $3 == "${TMV_THUMB_CLSID}"
        ClearErrors
        ReadRegStr $4 HKCU "${TMV_PLUGIN_RECORD}\$2" "ThumbnailBackup"
        ${If} ${Errors}
          DeleteRegValue HKCU "Software\Classes\.$2\ShellEx\${TMV_THUMB_SLOT}" ""
        ${Else}
          WriteRegStr HKCU "Software\Classes\.$2\ShellEx\${TMV_THUMB_SLOT}" "" "$4"
        ${EndIf}
      ${EndIf}
    ${EndIf}
    DeleteRegKey HKCU "${TMV_PLUGIN_RECORD}\$2"
  ${Loop}
  DeleteRegKey /ifempty HKCU "${TMV_PLUGIN_RECORD}"
  Pop $4
  Pop $3
  Pop $2
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Push $0
  Push $1
  SetRegView 64
  ; Built by scripts/build-thumbnails.mjs (beforeBundleCommand).
  SetOutPath "$INSTDIR"
  ; Explorer keeps a loaded DLL open: move it aside so the update can write.
  Delete "$INSTDIR\tmv_thumbnail_win.old"
  Rename "$INSTDIR\tmv_thumbnail_win.dll" "$INSTDIR\tmv_thumbnail_win.old"
  Delete /REBOOTOK "$INSTDIR\tmv_thumbnail_win.old"
  ; __FILEDIR__ is target\[<triple>\]release\nsis\x64 here, where Tauri writes installer.nsi.
  !if /FileExists "${__FILEDIR__}\..\..\..\thumbnail-windows\tmv_thumbnail_win.dll"
    File "${__FILEDIR__}\..\..\..\thumbnail-windows\tmv_thumbnail_win.dll"
  !else
    File "${__FILEDIR__}\..\..\..\..\thumbnail-windows\tmv_thumbnail_win.dll"
  !endif
  WriteRegStr HKCU "Software\Classes\CLSID\${TMV_THUMB_CLSID}" "" "TifMrcView thumbnails"
  WriteRegStr HKCU "Software\Classes\CLSID\${TMV_THUMB_CLSID}\InprocServer32" "" "$INSTDIR\tmv_thumbnail_win.dll"
  WriteRegStr HKCU "Software\Classes\CLSID\${TMV_THUMB_CLSID}\InprocServer32" "ThreadingModel" "Apartment"
  !insertmacro TMV_EACH_EXT TMV_REGISTER_EXT
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  Pop $1
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  Push $1
  SetRegView 64
  !insertmacro TMV_UNREGISTER_PLUGINS
  !insertmacro TMV_EACH_EXT TMV_UNREGISTER_EXT
  DeleteRegKey HKCU "Software\Classes\CLSID\${TMV_THUMB_CLSID}"
  DeleteRegKey /ifempty HKCU "${TMV_THUMB_BACKUP}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ; Explorer may still hold the DLL; remove it at the next reboot then.
  Delete /REBOOTOK "$INSTDIR\tmv_thumbnail_win.dll"
  Pop $1
  Pop $0
!macroend
