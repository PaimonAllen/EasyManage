# Read-only regression. Environment changes apply only to this test process.
param([Parameter(Mandatory=$true)][string]$DiscoveryPath)
$ErrorActionPreference='Stop'
. $DiscoveryPath
$checks=@{count=0}
function Check([bool]$ok,[string]$name){if(!$ok){throw "discovery check failed: $name"};$checks.count++}
$normal=Find-EasyManagePwsh ''
Check ([IO.Path]::IsPathRooted($normal)) 'absolute result'
$major=& $normal -NoProfile -NonInteractive -Command '$PSVersionTable.PSVersion.Major'
Check ([int]$major -ge 7) 'PowerShell 7+'
Check ((Find-EasyManagePwsh $normal) -eq $normal) 'explicit path'
$oldPath=$env:PATH;$oldLocal=$env:LOCALAPPDATA
try {
  $env:PATH="$env:SystemRoot\System32"
  $fallback=Find-EasyManagePwsh 'C:\EasyManage-nonexistent-fixture\pwsh.exe'
  Check ([IO.Path]::IsPathRooted($fallback)) 'missing PATH and stale hint'
  $env:LOCALAPPDATA=''
  $registered=Find-EasyManagePwsh ''
  Check ([IO.Path]::IsPathRooted($registered)) 'machine or current-user install record'
}finally{$env:PATH=$oldPath;$env:LOCALAPPDATA=$oldLocal}
$machine=$null;try{$machine=Find-EasyManagePwsh $normal $true}catch{}
Check (!$machine -or $machine.Replace('/','\') -notmatch '(?i)\\WindowsApps\\') 'LocalSystem never selects MSIX'
if($normal.Replace('/','\') -match '(?i)\\WindowsApps\\') {Check ($machine -ne $normal) 'user package not pinned to service'}
@{machine=$machine;checks=$checks.count;major=[int]$major;normal=$normal;fallback=$fallback;registered=$registered}|ConvertTo-Json -Compress
