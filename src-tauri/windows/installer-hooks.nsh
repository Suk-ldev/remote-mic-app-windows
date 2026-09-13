!include WinVer.nsh

!define SAYALL_MINIMUM_WINDOWS_BUILD 17763
!define SAYALL_DOWNGRADE_ERROR_LEVEL 1638
!define SAYALL_VB_CABLE_SERVICE_KEY "SYSTEM\CurrentControlSet\Services\VBAudioVACMME"
!define SAYALL_VB_CABLE_DOWNLOAD_URL "https://vb-audio.com/Cable/"
# RC003 返回/音量± 钩子需要注入 WUDFHost（管理员）；应用经此任务以最高权限
# 自启动（方案 C"默认管理员启动"，用户选定）。任务名固定 ASCII，避免代码页问题。
!define SAYALL_STARTUP_TASK "SayAllAdminStart"

!macro NSIS_HOOK_PREINSTALL
  ${IfNot} ${AtLeastBuild} ${SAYALL_MINIMUM_WINDOWS_BUILD}
    MessageBox MB_ICONSTOP|MB_OK "无线麦 SayAll 需要 Windows 10 1809（内部版本 17763）或更高版本。$\r$\nSayAll requires Windows 10 1809 (build 17763) or later."
    SetErrorLevel 1633
    Quit
  ${EndIf}

  Push $R8
  Push $R9
  ReadRegStr $R8 SHCTX "${UNINSTKEY}" "DisplayVersion"
  ${If} $R8 != ""
    nsis_tauri_utils::SemverCompare "${VERSION}" $R8
    Pop $R9
    ${If} $R9 = -1
      ${IfNot} ${Silent}
        MessageBox MB_ICONSTOP|MB_OK "已安装较新版本的无线麦 SayAll，不能用此旧版本覆盖。$\r$\nA newer version of SayAll is already installed. This older installer cannot replace it."
      ${EndIf}
      SetErrorLevel ${SAYALL_DOWNGRADE_ERROR_LEVEL}
      Quit
    ${EndIf}
  ${EndIf}
  Pop $R9
  Pop $R8
!macroend

!macro NSIS_HOOK_POSTINSTALL
  # 以最高权限的登录任务承载自启动（管理员用户免 UAC；注入 WUDFHost 需要权限）。
  # /f 覆盖旧任务（升级时刷新安装路径）；失败不阻断安装——应用仍可普通启动。
  nsExec::ExecToLog 'schtasks /create /f /tn "${SAYALL_STARTUP_TASK}" /tr "$\"$INSTDIR\无线麦 SayAll.exe$\"" /sc onlogon /rl HIGHEST'
  Pop $R8
  ${If} $R8 != 0
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION|MB_OK "自启动任务（管理员权限）注册失败（错误码 $R8）。返回/音量± 键钩子需要管理员权限才能生效；可稍后手动以管理员身份运行应用，或重新安装。$\r$\nThe admin autostart task could not be registered (error $R8). The back/volume hook needs admin rights; run the app as administrator later, or reinstall."
    ${EndIf}
  ${EndIf}
  Push $R8
  ReadRegStr $R8 HKLM "${SAYALL_VB_CABLE_SERVICE_KEY}" "DisplayName"
  ${If} $R8 == ""
    ${IfNot} ${Silent}
      MessageBox MB_ICONINFORMATION|MB_YESNO "无线麦需要 VB-CABLE 把遥控器语音传给输入法和语音软件。VB-CABLE 由 VB-Audio 提供，属于 Donationware，安装需要管理员权限，完成后必须重启 Windows。$\r$\n$\r$\n是否现在打开 VB-CABLE 官方下载页面？$\r$\n$\r$\nSayAll requires VB-CABLE for speech input. Installation requires administrator permission and a Windows restart. Open the official download page now?" IDYES sayall_vb_cable_open IDNO sayall_vb_cable_done
sayall_vb_cable_open:
      ExecShell "open" "${SAYALL_VB_CABLE_DOWNLOAD_URL}"
sayall_vb_cable_done:
    ${EndIf}
  ${EndIf}
  Pop $R8
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  # 移除管理员自启动任务；失败不阻断卸载。
  nsExec::ExecToLog 'schtasks /delete /f /tn "${SAYALL_STARTUP_TASK}"'
  Pop $R8
!macroend
