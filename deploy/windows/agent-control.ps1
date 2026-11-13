# Fixed pwsh operations. The controller supplies $request through SSH stdin.
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
function Is-Admin { ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) }
function Checked-Path([string]$path) {
 $full=[IO.Path]::GetFullPath($path)
 if($full.StartsWith('\\') -or $full -match '[\r\n]'){throw 'Local absolute path required'}
 $part=$full
 while($part){if(Test-Path -LiteralPath $part){if((Get-Item -LiteralPath $part -Force).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Reparse point refused'}};$next=[IO.Path]::GetDirectoryName($part);if($next -eq $part){break};$part=$next}
 return $full
}
function Private-Directory([string]$path,[bool]$full) {
 $path=Checked-Path $path
 if(!(Test-Path -LiteralPath $path)){New-Item -ItemType Directory -Path $path|Out-Null}
 $acl=[Security.AccessControl.DirectorySecurity]::new();$acl.SetAccessRuleProtection($true,$false)
 $owner=if($full){[Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')}else{[Security.Principal.WindowsIdentity]::GetCurrent().User};$acl.SetOwner($owner)
 $ids=@('S-1-5-18','S-1-5-32-544')
 if(!$full){$ids+=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value}
 foreach($id in $ids){$sid=[Security.Principal.SecurityIdentifier]::new($id);$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'))}
 # Owner Rights prevents a non-elevated file creator from granting itself write access.
 if($full){$acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-3-4'),'ReadPermissions','ContainerInherit,ObjectInherit','None','Allow'))}
 Set-Acl -LiteralPath $path -AclObject $acl
}
function Env-Value([string]$content,[string]$key) {
 $found=[regex]::Matches($content,('(?m)^'+[regex]::Escape($key)+'=(.*)\r?$'))
 if($found.Count -ne 1){throw 'Missing or duplicate Agent configuration key'}
 $value=$found[0].Groups[1].Value.Trim()
 if($value.StartsWith("'") -and $value.EndsWith("'")){$value=$value.Substring(1,$value.Length-2)}
 elseif($value.StartsWith('"') -and $value.EndsWith('"')){$value=$value.Substring(1,$value.Length-2)}
 return $value
}
function Read-Identity([string]$root) {
 $node=Get-Content -LiteralPath (Join-Path $root 'config/node.json') -Raw|ConvertFrom-Json
 if($node.node_id -ne $request.node_id -or $node.mode -ne $request.mode){throw 'Original identity or privilege differs'}
 $content=Get-Content -LiteralPath (Join-Path $root 'config/agent.env') -Raw
 if((Env-Value $content 'EASYMANAGE_NODE_ID') -ne $request.node_id){throw 'Original configured ID differs'}
 $state=(Join-Path $root 'state/state.db').Replace('\','/')
 if(![string]::Equals((Env-Value $content 'EASYMANAGE_AGENT_STATE_DB'),$state,[StringComparison]::OrdinalIgnoreCase)){throw 'Original state path differs'}
}
function Task-Name { 'EasyManageAgent-'+[Security.Principal.WindowsIdentity]::GetCurrent().User.Value }
function Assert-OwnedTask($task,[string]$root) {
 $actions=@($task.Actions)
 $exe=Join-Path $root 'bin/easymanage-agent.exe'
 $args='--config "'+(Join-Path $root 'config/agent.env')+'"'
 if($actions.Count -ne 1 -or ![string]::Equals($actions[0].Execute,$exe,[StringComparison]::OrdinalIgnoreCase) -or ![string]::Equals($actions[0].Arguments,$args,[StringComparison]::OrdinalIgnoreCase)){throw 'Unrelated scheduled task refused'}
}
function Stop-Owned([string]$root,[bool]$full) {
 if($full){
  $service=Get-CimInstance Win32_Service -Filter "Name='EasyManageAgent'"
  if($service){if(![string]::Equals($service.PathName,('"'+(Join-Path $root 'bin/easymanage-agent.exe')+'" --service --config "'+(Join-Path $root 'config/agent.env')+'"'),[StringComparison]::OrdinalIgnoreCase)){throw 'Unrelated service refused'}
   if($service.State -ne 'Stopped'){Stop-Service EasyManageAgent;(Get-Service EasyManageAgent).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(20))}}
 }else{
  $task=Get-ScheduledTask -TaskName (Task-Name) -ErrorAction SilentlyContinue
  if($task){Assert-OwnedTask $task $root;Stop-ScheduledTask -TaskName (Task-Name);for($i=0;$i -lt 40;$i++){if((Get-ScheduledTask -TaskName (Task-Name)).State -ne 'Running'){break};Start-Sleep -Milliseconds 250};if((Get-ScheduledTask -TaskName (Task-Name)).State -eq 'Running'){throw 'Task did not stop'}}
 }
}
function Start-Owned([string]$root,[bool]$full) {
 if($full){Set-Service EasyManageAgent -StartupType Automatic;Start-Service EasyManageAgent;(Get-Service EasyManageAgent).WaitForStatus('Running',[TimeSpan]::FromSeconds(20))}
 else{Assert-OwnedTask (Get-ScheduledTask -TaskName (Task-Name)) $root;Enable-ScheduledTask -TaskName (Task-Name)|Out-Null;Start-ScheduledTask -TaskName (Task-Name)}
}
try {
 if($request.action -eq 'inspect'){
  if(!$IsWindows -or $PSVersionTable.PSVersion.Major -lt 7){throw 'Windows and PowerShell 7 required'}
  $id=[Security.Principal.WindowsIdentity]::GetCurrent()
  if($request.mode -eq 'full' -and !(Is-Admin)){throw 'Full requires an elevated SSH token'}
  $servicePwsh=$null
  if($request.mode -eq 'full'){try{$servicePwsh=Find-EasyManagePwsh ((Get-Process -Id $PID).Path) $true}catch{}}
  $root=if($request.mode -eq 'full'){Join-Path $env:ProgramData 'EasyManage'}else{Join-Path $env:LOCALAPPDATA 'EasyManage'}
  @{platform='windows';username=$id.Name;principal=$id.User.Value;administrator=(Is-Admin);home=(Checked-Path $root).Replace('\','/');architecture=[Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant();hostname=$env:COMPUTERNAME;pwsh=(Get-Process -Id $PID).Path;service_pwsh=$servicePwsh}|ConvertTo-Json -Compress
  exit 0
 }
 if($request.action -eq 'verify_admin'){if(!(Is-Admin)){throw 'Administrator token required'};'verified';exit 0}
 if($request.action -eq 'verify_loopback'){
  $listeners=@(Get-NetTCPConnection -State Listen -LocalPort ([int]$request.port))
  if(!$listeners.Count -or @($listeners|Where-Object LocalAddress -NotIn @('127.0.0.1','::1')).Count){throw 'Forward not loopback-only'}
  'loopback';exit 0
 }
 $full=$request.mode -eq 'full'
 $root=Checked-Path $request.base
 $expected=if($full){Join-Path $env:ProgramData 'EasyManage'}else{Join-Path $env:LOCALAPPDATA 'EasyManage'}
 if(![string]::Equals($root.TrimEnd('\'),[IO.Path]::GetFullPath($expected).TrimEnd('\'),[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected managed root'}
 if($full -and !(Is-Admin)){throw 'Administrator token required'}
 switch($request.action){
  'prepare' {
   if($request.repair){Read-Identity $root}
   elseif((Test-Path -LiteralPath (Join-Path $root 'config/node.json')) -or (Test-Path -LiteralPath (Join-Path $root 'config/agent.env')) -or (Test-Path -LiteralPath (Join-Path $root 'bin/easymanage-agent.exe')) -or (Test-Path -LiteralPath (Join-Path $root 'state/state.db')) -or (Get-Service EasyManageAgent -ErrorAction SilentlyContinue)){throw 'Agent exists; use original node'}
   if(!$request.repair -and !$request.promotion){if(Get-ScheduledTask -TaskName (Task-Name) -ErrorAction SilentlyContinue){throw 'Existing user Agent task refused'};if($full -and (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'EasyManage/config/agent.env'))){throw 'Existing Basic identity refused'}}
   if($request.stage -notmatch '^agent\.[a-f0-9]{32}$'){throw 'Invalid staging identifier'}
   if(!$request.repair -and (Test-Path -LiteralPath $root) -and @(Get-ChildItem -LiteralPath $root -Force).Count){throw 'New installation requires an empty managed directory; retained files must be reviewed'}
   Private-Directory $root $full
   Private-Directory (Join-Path $root 'staging') $full
   $stage=Join-Path $root ('staging/'+$request.stage)
   if(Test-Path -LiteralPath $stage){throw 'Staging collision'}
   Private-Directory $stage $full;$stage.Replace('\','/')
  }
  'install' {
   if($request.stage -notmatch '^agent\.[a-f0-9]{32}$'){throw 'Invalid staging identifier'}
   $stage=Checked-Path (Join-Path $root ('staging/'+$request.stage))
   foreach($name in @('easymanage-agent.exe','agent.env','ca.crt','node.json')){
    if((Get-FileHash -LiteralPath (Join-Path $stage $name) -Algorithm SHA256).Hash.ToLowerInvariant() -ne $request.hashes.$name){throw 'Staged hash mismatch'}
   }
   if(!$request.repair){if(Get-Service EasyManageAgent -ErrorAction SilentlyContinue){throw 'Service appeared; original node required'};if(Get-ScheduledTask -TaskName (Task-Name) -ErrorAction SilentlyContinue){throw 'Task appeared; original node required'}}
   if($request.repair){Read-Identity $root;Stop-Owned $root $full}
   elseif(Test-Path -LiteralPath (Join-Path $root 'config/node.json')){throw 'Identity appeared'}
   foreach($dir in @('bin','config','state','logs')){Private-Directory (Join-Path $root $dir) $full}
   $exe=Join-Path $root 'bin/easymanage-agent.exe';$config=Join-Path $root 'config/agent.env'
   foreach($file in @($exe,$config)){if(Test-Path -LiteralPath $file){Copy-Item -LiteralPath $file -Destination ($file+'.previous') -Force}}
   Copy-Item -LiteralPath (Join-Path $stage 'easymanage-agent.exe') -Destination $exe -Force
   foreach($name in @('agent.env','ca.crt','node.json')){Copy-Item -LiteralPath (Join-Path $stage $name) -Destination (Join-Path $root ('config/'+$name)) -Force}
   if($full){
    $command='"'+$exe+'" --service --config "'+$config+'"'
    if(!(Get-Service EasyManageAgent -ErrorAction SilentlyContinue)){New-Service -Name EasyManageAgent -BinaryPathName $command -DisplayName 'EasyManage Agent' -StartupType Automatic|Out-Null}
    else{& sc.exe config EasyManageAgent binPath= $command|Out-Null;if($LASTEXITCODE){throw 'SCM configuration failed'}}
    & sc.exe failure EasyManageAgent reset= 86400 actions= restart/10000/restart/30000/restart/60000|Out-Null
    if($LASTEXITCODE){throw 'SCM recovery configuration failed'}
   }else{
    $action=New-ScheduledTaskAction -Execute $exe -Argument ('--config "'+$config+'"') -WorkingDirectory $root
    $principal=New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
    $settings=New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
    $trigger=New-ScheduledTaskTrigger -AtLogOn -User $principal.UserId
    Register-ScheduledTask -TaskName (Task-Name) -Action $action -Principal $principal -Trigger $trigger -Settings $settings -Force|Out-Null
   }
   try{Start-Owned $root $full}catch{
    Stop-Owned $root $full
    foreach($file in @($exe,$config)){if(Test-Path -LiteralPath ($file+'.previous')){Copy-Item -LiteralPath ($file+'.previous') -Destination $file -Force}}
    throw 'Start failed; previous files retained; verify service state'
   }
   'installed'
  }
  'promote' {
   if(!$full -or !(Is-Admin)){throw 'Administrator token required'}
   $source=Checked-Path $request.source_base
   if(!$source.Replace('\','/').EndsWith('/AppData/Local/EasyManage',[StringComparison]::OrdinalIgnoreCase)){throw 'Unexpected Basic installation'}
   foreach($item in @('config/node.json','config/agent.env','config/ca.crt','state/state.db')){[void](Checked-Path (Join-Path $source $item))}
   $node=Get-Content -LiteralPath (Join-Path $source 'config/node.json') -Raw|ConvertFrom-Json
   if($node.node_id -ne $request.node_id -or $node.mode -ne 'basic' -or !$request.source_principal -or $node.principal -ne $request.source_principal){throw 'Original Basic identity differs'}
   if((Get-FileHash -LiteralPath (Join-Path $source 'config/ca.crt') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $request.ca_sha256){throw 'Original CA differs'}
   if($request.stage -notmatch '^agent\.[a-f0-9]{32}$'){throw 'Invalid stage'}
   $stage=Checked-Path (Join-Path $root ('staging/'+$request.stage));$binary=Join-Path $stage 'easymanage-agent.exe'
   if((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $request.sha256){throw 'Candidate differs'}
   if((Get-Service EasyManageAgent -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath (Join-Path $root 'state/state.db'))){throw 'Full installation already exists; do not replace identity'}
   $taskName='EasyManageAgent-'+$request.source_principal
   $task=Get-ScheduledTask -TaskName $taskName
   Assert-OwnedTask $task $source
   Stop-ScheduledTask -TaskName $taskName
   for($i=0;$i -lt 40;$i++){if((Get-ScheduledTask -TaskName $taskName).State -ne 'Running'){break};Start-Sleep -Milliseconds 250}
   if((Get-ScheduledTask -TaskName $taskName).State -eq 'Running'){throw 'Basic task did not stop'}
   Disable-ScheduledTask -TaskName $taskName|Out-Null
   try {
    foreach($dir in @('bin','config','state','logs')){Private-Directory (Join-Path $root $dir) $true}
    foreach($suffix in @('','-wal')){
     $file=Join-Path $source ('state/state.db'+$suffix)
     if(Test-Path -LiteralPath $file){[void](Checked-Path $file);Copy-Item -LiteralPath $file -Destination (Join-Path $root ('state/state.db'+$suffix))}
    }
    $content=Get-Content -LiteralPath (Join-Path $source 'config/agent.env') -Raw
    $old=(Join-Path $source 'state/state.db').Replace('\','/')
    if(![string]::Equals((Env-Value $content 'EASYMANAGE_AGENT_STATE_DB'),$old,[StringComparison]::OrdinalIgnoreCase)){throw 'Original state path differs'}
    $content=[regex]::Replace($content,'(?m)^EASYMANAGE_AGENT_STATE_DB=.*$',("EASYMANAGE_AGENT_STATE_DB='"+$root.Replace('\','/')+"/state/state.db'"))
    $content=[regex]::Replace($content,'(?m)^EASYMANAGE_CA_CERTIFICATE=.*$',("EASYMANAGE_CA_CERTIFICATE='"+$root.Replace('\','/')+"/config/ca.crt'"))
    if(!$request.pwsh){throw 'Machine-wide PowerShell path required'}
    if($content -match '(?m)^EASYMANAGE_PWSH_PATH='){$content=[regex]::Replace($content,'(?m)^EASYMANAGE_PWSH_PATH=.*$',("EASYMANAGE_PWSH_PATH='"+$request.pwsh+"'"))}
    else{$content="EASYMANAGE_PWSH_PATH='"+$request.pwsh+"'`n"+$content}
    $config=Join-Path $root 'config/agent.env';[IO.File]::WriteAllText($config,$content,[Text.UTF8Encoding]::new($false))
    Copy-Item -LiteralPath (Join-Path $source 'config/ca.crt') -Destination (Join-Path $root 'config/ca.crt')
    $node.mode='full';[IO.File]::WriteAllText((Join-Path $root 'config/node.json'),($node|ConvertTo-Json -Compress),[Text.UTF8Encoding]::new($false))
    $exe=Join-Path $root 'bin/easymanage-agent.exe';Copy-Item -LiteralPath $binary -Destination $exe
    New-Service -Name EasyManageAgent -BinaryPathName ('"'+$exe+'" --service --config "'+$config+'"') -DisplayName 'EasyManage Agent' -StartupType Automatic|Out-Null
    Start-Owned $root $true
   }catch{
    # Preserve both state copies; never start both identities after an uncertain SCM result.
    $service=Get-Service EasyManageAgent -ErrorAction SilentlyContinue
    if($service -and $service.Status -eq 'Running'){throw 'Full may be running; original Basic remains disabled; verify actual heartbeat'}
    Enable-ScheduledTask -TaskName $taskName|Out-Null;Start-ScheduledTask -TaskName $taskName
    throw 'Promotion failed; original Basic task restored and both state copies retained'
   }
   'promoted'
  }
  'restart' {Read-Identity $root;Stop-Owned $root $full;Start-Owned $root $full;'restarted'}
  'force_restart' {
   Read-Identity $root
   if(!(Is-Admin)){throw 'Administrator token required'}
   if($full){
    $service=Get-CimInstance Win32_Service -Filter "Name='EasyManageAgent'"
    $expected='"'+(Join-Path $root 'bin/easymanage-agent.exe')+'" --service --config "'+(Join-Path $root 'config/agent.env')+'"'
    if(!$service -or ![string]::Equals($service.PathName,$expected,[StringComparison]::OrdinalIgnoreCase)){throw 'Unrelated service refused'}
    if($service.ProcessId){
     $process=Get-Process -Id $service.ProcessId
     if(![string]::Equals($process.Path,(Join-Path $root 'bin/easymanage-agent.exe'),[StringComparison]::OrdinalIgnoreCase)){throw 'Process ownership differs'}
     Set-Service EasyManageAgent -StartupType Manual
     Stop-Process -Id $service.ProcessId -Force
     (Get-Service EasyManageAgent).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(20))
    }
   }else{Stop-Owned $root $false}
   Start-Owned $root $full;'restarted'
  }

  'stop' {Read-Identity $root;Stop-Owned $root $full;if($full){Set-Service EasyManageAgent -StartupType Disabled}else{Disable-ScheduledTask -TaskName (Task-Name)|Out-Null};'AGENT_STOPPED_FILES_RETAINED'}
  'configure' {
   Read-Identity $root;$config=Join-Path $root 'config/agent.env'
   if((Get-FileHash -LiteralPath (Join-Path $root 'config/ca.crt') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $request.ca_sha256){throw 'CA differs'}
   if($request.url -notmatch '^https://[^\s\r\n]+$'){throw 'Invalid HTTPS URL'}
   $content=Get-Content -LiteralPath $config -Raw
   $content=[regex]::Replace($content,'(?m)^EASYMANAGE_SERVER_URL=.*$',('EASYMANAGE_SERVER_URL='+$request.url))
   if($request.token){$content=[regex]::Replace($content,'(?m)^EASYMANAGE_ENROLLMENT_TOKEN=.*$',('EASYMANAGE_ENROLLMENT_TOKEN='+$request.token))}
   Copy-Item -LiteralPath $config -Destination ($config+'.previous') -Force
   [IO.File]::WriteAllText($config+'.next',$content,[Text.UTF8Encoding]::new($false));Move-Item -LiteralPath ($config+'.next') -Destination $config -Force;'configured'
  }
  'update' {
   Read-Identity $root
   if($request.stage -notmatch '^agent\.[a-f0-9]{32}$'){throw 'Invalid staging identifier'}
   $stage=Checked-Path (Join-Path $root ('staging/'+$request.stage));$source=Join-Path $stage 'easymanage-agent.exe'
   if((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant() -ne $request.sha256){throw 'Update hash differs'}
   $exe=Join-Path $root 'bin/easymanage-agent.exe';Stop-Owned $root $full;Copy-Item -LiteralPath $exe -Destination ($exe+'.previous') -Force
   try{Copy-Item -LiteralPath $source -Destination $exe -Force;Start-Owned $root $full}catch{Copy-Item -LiteralPath ($exe+'.previous') -Destination $exe -Force;Start-Owned $root $full;throw 'Update failed; previous binary restored'}
   'updated'
  }
  'cleanup' {
   if($request.stage -notmatch '^agent\.[a-f0-9]{32}$'){throw 'Invalid staging identifier'}
   $stage=Checked-Path (Join-Path $root ('staging/'+$request.stage))
   foreach($name in @('easymanage-agent.exe','agent.env','ca.crt','node.json')){$file=Join-Path $stage $name;if(Test-Path -LiteralPath $file){[IO.File]::Delete($file)}}
   [IO.Directory]::Delete($stage,$false);'cleaned'
  }
  default {throw 'Unsupported Windows operation'}
 }
}catch{[Console]::Error.WriteLine('EasyManage Windows operation failed; check pwsh, identity, permissions and retained staging files');exit 1}
