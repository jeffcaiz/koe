[Setup]
AppName=Koe
AppVersion={#GetEnv('KOE_VERSION')}
AppPublisher=jeffcaiz
AppPublisherURL=https://github.com/jeffcaiz/koe
DefaultDirName={autopf}\Koe
DefaultGroupName=Koe
UninstallDisplayIcon={app}\koe.exe
OutputDir=..\output
OutputBaseFilename=koe-{#GetEnv('KOE_VERSION')}-x86_64-windows-setup
SetupIconFile=..\koe-shell\assets\koe.ico
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest

[Files]
Source: "..\target\x86_64-pc-windows-msvc\release\koe.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autodesktop}\Koe"; Filename: "{app}\koe.exe"; IconFilename: "{app}\koe.exe"; Comment: "Koe Voice Input"
Name: "{group}\Koe"; Filename: "{app}\koe.exe"; IconFilename: "{app}\koe.exe"
Name: "{group}\Uninstall Koe"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\koe.exe"; Description: "Launch Koe"; Flags: nowait postinstall skipifsilent
