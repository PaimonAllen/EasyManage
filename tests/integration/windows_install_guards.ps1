# Pure synthetic ACL/task guard tests. No files, services, accounts or tasks are changed.
param([Parameter(Mandatory=$true)][string]$DiscoveryPath,[Parameter(Mandatory=$true)][string]$ControlPath,[Parameter(Mandatory=$true)][string]$FixtureRoot)
$ErrorActionPreference='Stop'
$checks=@{count=0}
function Check([bool]$ok,[string]$name){if(!$ok){throw "guard failed: $name"};$checks.count++}
function Function-Source([string]$path,[string]$name) {
 $tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseInput([IO.File]::ReadAllText($path),[ref]$tokens,[ref]$errors)
 if($errors.Count){throw 'PowerShell parse errors'}
 ($ast.Find({param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq $name},$true)).Extent.Text
}
$machineSource=Function-Source $DiscoveryPath 'Test-MachinePwsh'
$taskSource=Function-Source $ControlPath 'Assert-OwnedTask'
& {
 . ([scriptblock]::Create($machineSource))
 $fixture=@{owner='S-1-5-18';rules=@();reparse=$false}
 function Get-Item {param($LiteralPath,[switch]$Force) @{Attributes=$(if($fixture.reparse){[IO.FileAttributes]::ReparsePoint}else{[IO.FileAttributes]::Normal})}}
 function Get-Acl {param($LiteralPath)
  $acl=[pscustomobject]@{}
  $acl|Add-Member ScriptMethod GetOwner {param($type) @{Value=$fixture.owner}}
  $acl|Add-Member ScriptMethod GetAccessRules {param($a,$b,$c) $fixture.rules}
  return $acl
 }
 function Rule([string]$sid,[int]$rights,[int]$prop=0){[pscustomobject]@{AccessControlType='Allow';PropagationFlags=$prop;FileSystemRights=$rights;IdentityReference=@{Value=$sid}}}
 $p='C:\ProtectedFixture\pwsh.exe'
 Check (Test-MachinePwsh $p) 'trusted owner'
 $fixture.rules=@((Rule 'S-1-5-32-545' 0x200A9));Check (Test-MachinePwsh $p) 'read execute allowed'
 $fixture.rules=@((Rule 'S-1-5-32-545' 2));Check (!(Test-MachinePwsh $p)) 'unprivileged writer refused'
 $fixture.rules=@();$fixture.owner='S-1-5-21-1234';Check (!(Test-MachinePwsh $p)) 'user owner refused'
 $fixture.owner='S-1-5-18';$fixture.reparse=$true;Check (!(Test-MachinePwsh $p)) 'reparse refused'
 $fixture.reparse=$false;Check (!(Test-MachinePwsh 'C:\Program Files\WindowsApps\Fixture\pwsh.exe')) 'MSIX refused for service'
 $fixture.rules=@((Rule 'S-1-5-32-545' 2 2));Check (Test-MachinePwsh $p) 'inherit only ACE ignored'
}
. ([scriptblock]::Create($taskSource))
$root='C:\SyntheticAgent'
$task=[pscustomobject]@{Actions=@([pscustomobject]@{Execute=(Join-Path $root 'bin/easymanage-agent.exe');Arguments=('--config "'+(Join-Path $root 'config/agent.env')+'"')})}
Assert-OwnedTask $task $root;Check $true 'owned task accepted'
$task.Actions[0].Arguments='--config "C:\another-node\agent.env"'
$refused=$false;try{Assert-OwnedTask $task $root}catch{$refused=$true};Check $refused 'other config refused'
$task.Actions[0].Execute='C:\business.exe'
$refused=$false;try{Assert-OwnedTask $task $root}catch{$refused=$true};Check $refused 'business executable refused'
$privateSource=Function-Source $ControlPath 'Private-Directory'
& {
 . ([scriptblock]::Create($privateSource))
 $captured=@{acl=$null}
 function Checked-Path([string]$path){return $path}
 function Test-Path {param($LiteralPath) return $true}
 function New-Item {throw 'Filesystem mutation forbidden in ACL fixture'}
 function Set-Acl {param($LiteralPath,$AclObject) $captured.acl=$AclObject}
 Private-Directory 'C:\SyntheticAcl' $true
 Check ($captured.acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -eq 'S-1-5-32-544') 'Full directory owned by Administrators'
 $rules=@($captured.acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]))
 $ownerRights=@($rules|Where-Object {$_.IdentityReference.Value -eq 'S-1-3-4'})
 Check ($ownerRights.Count -eq 1 -and !([int]$ownerRights[0].FileSystemRights -band 0x40000)) 'creator cannot recover implicit WRITE_DAC'
 Check (@($rules|Where-Object {$_.IdentityReference.Value -notin @('S-1-5-18','S-1-5-32-544','S-1-3-4')}).Count -eq 0) 'Full does not grant login SID write'
 Private-Directory 'C:\SyntheticAcl' $false
 Check ($captured.acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -eq [Security.Principal.WindowsIdentity]::GetCurrent().User.Value) 'Basic keeps user ownership'
}
$envSource=Function-Source $ControlPath 'Env-Value'
. ([scriptblock]::Create($envSource))
foreach($value in @("C:/Program Files/EasyManage/state/state.db","C:/Users/Fixture/AppData/Local/EasyManage/state/state.db")) {
 Check ((Env-Value ("KEY='"+$value+"'`r`n") 'KEY') -eq $value) 'quoted path preserved'
 Check ((Env-Value ("KEY="+$value+"`n") 'KEY') -eq $value) 'legacy path accepted'
}
$refused=$false;try{[void](Env-Value "KEY=first`nKEY=second`n" 'KEY')}catch{$refused=$true};Check $refused 'ambiguous identity config refused'
$fixtureFull=[IO.Path]::GetFullPath($FixtureRoot)
$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
if(!$fixtureFull.StartsWith($temp,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($fixtureFull) -notmatch '^EasyManageReadonly-[a-f0-9]{32}$' -or ((Get-Item -LiteralPath $fixtureFull).Attributes -band [IO.FileAttributes]::ReparsePoint)){throw 'Fresh guarded temp root required'}
$nonempty=Join-Path $fixtureFull ('nonempty-'+[guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($nonempty);[IO.File]::WriteAllText((Join-Path $nonempty 'unrelated-fixture.txt'),'synthetic')
$aclBefore=(Get-Acl -LiteralPath $nonempty).Sddl
$tokens=$null;$errors=$null;$ast=[Management.Automation.Language.Parser]::ParseInput([IO.File]::ReadAllText($ControlPath),[ref]$tokens,[ref]$errors)
$switch=$ast.Find({param($n) $n -is [Management.Automation.Language.SwitchStatementAst]},$true)
$clause=($switch.Clauses|Where-Object {$_.Item1.Value -eq 'prepare'}).Item2.Extent.Text
$prepare=[scriptblock]::Create($clause.Substring(1,$clause.Length-2))
& {
 $request=[pscustomobject]@{repair=$false;promotion=$false;stage=('agent.'+[guid]::NewGuid().ToString('N'))};$full=$true;$root=$nonempty
 function Get-Service {param($Name,$ErrorAction) return $null}
 function Get-ScheduledTask {param($TaskName,$ErrorAction) return $null}
 function Task-Name {'synthetic'}
 function Private-Directory {throw 'ACL mutation reached'}
 $reason='';try{& $prepare}catch{$reason=$_.Exception.Message}
 Check ($reason -like '*empty managed directory*') 'nonempty root refused before ACL mutation'
}
Check (((Get-Acl -LiteralPath $nonempty).Sddl -eq $aclBefore) -and [IO.File]::ReadAllText((Join-Path $nonempty 'unrelated-fixture.txt')) -eq 'synthetic') 'fixture ACL and unrelated file unchanged'
@{checks=$checks.count;synthetic=$true}|ConvertTo-Json -Compress
