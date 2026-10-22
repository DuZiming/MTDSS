; MTDSS NSIS 安装器生命周期钩子(D11/D14)。
; 经 tauri.conf.json > bundle > windows > nsis > installerHooks 挂入 tauri 模板;
; 四个宏按点插入:PREINSTALL=复制文件前、POSTINSTALL=复制/注册表/快捷方式后、
; PREUNINSTALL=删除文件前、POSTUNINSTALL=删除文件后。
;
; 进程探测用的是 tauri 工具链自带的 nsis_tauri_utils 聚合插件(模板经
; !addplugindir ${ADDITIONALPLUGINSPATH} 已使其可编译/可用):
;   - FindProcess "x.exe":栈压 0=找到 / 1=未找到(大小写不敏感);
;   - (KillProcess "x.exe":0=已杀 / 1=部分未杀 / 2=未找到;本次未用,强杀走
;     nsExec taskkill /F,与任务口径一致。)
;   旧版 tauri 文档提到的 nsProcess 插件不在其 NSIS 工具链(nsis-3.13.zip)内。
;
; 与模板自身行为的关系:模板在 PREINSTALL/PREUNINSTALL 钩子之后各有一次
; CheckIfAppIsRunning(RestartManager 方式处理主程序 ui.exe),本文件的轮询/
; taskkill 与之幂等——不同阶段,先后都安全;mtdssd.exe 模板不认识,由本钩子负责。

!macro NSIS_HOOK_PREINSTALL
  ; D11:装前轮询等待 mtdssd.exe / ui.exe 退出,≤15s(500ms × 30);
  ; 超时 → taskkill /F 兜底后放行(上游空闲门已保证此刻无传输可杀)。
  DetailPrint "Waiting for MTDSS processes to exit (up to 15s)..."
  StrCpy $R9 0
  ${While} $R9 < 30
    StrCpy $R8 0
    nsis_tauri_utils::FindProcess "mtdssd.exe"
    Pop $R0
    ${If} $R0 = 0
      StrCpy $R8 1
    ${EndIf}
    nsis_tauri_utils::FindProcess "ui.exe"
    Pop $R0
    ${If} $R0 = 0
      StrCpy $R8 1
    ${EndIf}
    ${If} $R8 = 0
      ${ExitWhile}
    ${EndIf}
    Sleep 500
    IntOp $R9 $R9 + 1
  ${EndWhile}
  ${If} $R8 = 1
    DetailPrint "Timeout waiting for MTDSS processes; force killing..."
    nsExec::Exec 'taskkill /F /IM mtdssd.exe /T'
    Pop $R0
    nsExec::Exec 'taskkill /F /IM ui.exe /T'
    Pop $R0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; D11:新装/升级完成后拉起 daemon(托盘回归)。Exec 为 fire-and-forget,
  ; 不阻塞安装器收尾。
  Exec '"$INSTDIR\mtdssd.exe"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; D14:卸载前停进程 + 清自启 Run 键。值名 "MTDSS" 与 daemon 写入一致
  ; (crates/daemon/src/api.rs AUTOSTART_RUN_VALUE)。
  nsExec::Exec 'taskkill /F /IM mtdssd.exe /T'
  Pop $R0
  nsExec::Exec 'taskkill /F /IM ui.exe /T'
  Pop $R0
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MTDSS"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 有意留空:保留 MTDSS_HOME 下的用户数据(业界惯例);tauri 默认卸载器
  ; 本就只清 $INSTDIR 内容与(用户勾选时的)webview 数据目录,不会动
  ; $INSTDIR 之外的 MTDSS_HOME。
!macroend
