$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
try {
 $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
 if(!([Security.Principal.WindowsPrincipal]$identity).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)){throw 'Administrator token required'}
 $operation=$request.operation
 $adminGroup=Get-LocalGroup -SID 'S-1-5-32-544'
 $admins=@(Get-LocalGroupMember -Group $adminGroup.Name|ForEach-Object {$_.SID.Value})
 $users=@(Get-LocalUser)
 if($operation.action -eq 'list'){
  @{accounts=@($users|ForEach-Object {@{username=$_.Name;uid=[uint32]::MaxValue;gid=[uint32]::MaxValue;native_id=$_.SID.Value;home='';shell='pwsh';display_name=$_.FullName;account_status=$(if($_.Enabled){'normal'}else{'restricted'})}})}|ConvertTo-Json -Depth 6 -Compress;exit 0
 }
 $name=[string]$operation.subject
 if(!$name -or $name.Length -gt 20 -or $name -match '[\/:*?"<>|\x00-\x1f]' -or $name.EndsWith('.') -or $operation.confirmation -cne $name){throw 'Invalid account or confirmation'}
 $target=$users|Where-Object Name -EQ $name
 if($target){
  $sid=$target.SID.Value
  if([int]($sid.Split('-')[-1]) -lt 1000 -or $sid -eq $identity.User.Value -or $sid -in $admins){throw 'Built-in/current/administrator account protected'}
  if($operation.action -eq 'delete'){
   $running=@(Get-Process -IncludeUserName -ErrorAction Stop|Where-Object {$_.UserName -and ($_.UserName.Split('\')[-1] -ieq $name)})
   if($running.Count){throw 'Account has running processes'}
  }
 }
 $groups=@()
 if($null -ne $operation.groups){foreach($nameOfGroup in @($operation.groups)){$groups+=Get-LocalGroup -Name $nameOfGroup -ErrorAction Stop}}
 switch($operation.action){
  'create' {if($target){throw 'Account exists'};New-LocalUser -Name $name -FullName $operation.display_name -NoPassword -Disabled|Out-Null}
  'update' {if(!$target){throw 'Account missing'};Set-LocalUser -Name $name -FullName $operation.display_name}
  'delete' {if(!$target){throw 'Account missing'};Remove-LocalUser -Name $name}
  default {throw 'Unsupported action'}
 }
 if($operation.action -in @('create','update') -and $null -ne $operation.groups){
  $member=Get-LocalUser -Name $name
  # Explicitly requested group replacement, preserving the built-in Users membership.
  foreach($g in @(Get-LocalGroup)){
   if($g.SID.Value -eq 'S-1-5-32-545'){continue}
   $members=@(Get-LocalGroupMember -Group $g.Name)
   if($member.SID.Value -in @($members|ForEach-Object {$_.SID.Value}) -and $g.Name -notin @($groups|ForEach-Object {$_.Name})){Remove-LocalGroupMember -Group $g.Name -Member $member}
  }
  foreach($g in $groups){if($member.SID.Value -notin @(Get-LocalGroupMember -Group $g.Name|ForEach-Object {$_.SID.Value})){Add-LocalGroupMember -Group $g.Name -Member $member}}
 }
 @{status='ok';message='Windows local account operation completed. New accounts are disabled until an administrator configures authentication. User profiles/files are retained on deletion.'}|ConvertTo-Json -Compress
}catch{[Console]::Error.WriteLine('Windows account operation refused or failed; built-in/current/admin/active accounts are protected');exit 1}
