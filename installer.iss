; Solinear installer (Inno Setup 6). Built by build.py.
[Setup]
AppName=Solinear
AppVersion=0.1.0
DefaultDirName={autopf}\Solinear
DefaultGroupName=Solinear
PrivilegesRequired=admin
OutputDir=dist
OutputBaseFilename=SolinearSetup
Compression=lzma2
SolidCompression=yes
DisableProgramGroupPage=yes

[Files]
Source: "dist\Solinear\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion
; never ship the build machine's Moonlight identity (Moonlight.ini holds its private key), logs or cache
Source: "bin\*"; DestDir: "{app}\bin"; Excludes: "Moonlight Game Streaming Project,*.log,cache,portable.dat"; Flags: recursesubdirs ignoreversion

[Icons]
; Start menu shortcut runs the elevated task, so users never see a UAC prompt
Name: "{autoprograms}\Solinear"; Filename: "{sys}\schtasks.exe"; Parameters: "/run /tn Solinear"; IconFilename: "{app}\Solinear.exe"

[Run]
; Start elevated at logon (tailscaled needs admin for its network adapter)
; (PowerShell rather than schtasks: schtasks defaults stop the task on battery and after 72 h, which breaks laptops)
Filename: "powershell.exe"; Parameters: "-NoProfile -Command ""$a = New-ScheduledTaskAction -Execute '{app}\Solinear.exe'; $t = New-ScheduledTaskTrigger -AtLogOn; $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0; $p = New-ScheduledTaskPrincipal -GroupId 'BUILTIN\Users' -RunLevel Highest; Register-ScheduledTask -TaskName Solinear -Action $a -Trigger $t -Settings $s -Principal $p -Force"""; Flags: runhidden
; Streaming and pairing ports reachable only from the Tailscale network (100.64.0.0/10)
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""Solinear Apollo"" dir=in action=allow program=""{app}\bin\apollo\sunshine.exe"" remoteip=100.64.0.0/10 enable=yes"; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""Solinear Agent"" dir=in action=allow program=""{app}\Solinear.exe"" remoteip=100.64.0.0/10 enable=yes"; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""Solinear Net"" dir=in action=allow program=""{app}\bin\tailscale\tailscaled.exe"" enable=yes"; Flags: runhidden
Filename: "{sys}\schtasks.exe"; Parameters: "/run /tn Solinear"; Description: "Start Solinear"; Flags: postinstall runhidden nowait

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im Solinear.exe"; Flags: runhidden; RunOnceId: "Kill"
Filename: "{sys}\schtasks.exe"; Parameters: "/delete /f /tn Solinear"; Flags: runhidden; RunOnceId: "Task"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Solinear Apollo"""; Flags: runhidden; RunOnceId: "FwApollo"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Solinear Agent"""; Flags: runhidden; RunOnceId: "FwAgent"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Solinear Net"""; Flags: runhidden; RunOnceId: "FwNet"
