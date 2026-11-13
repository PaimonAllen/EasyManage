# Windows PowerShell 5.1-compatible discovery only; workloads always run in pwsh 7+.
function Find-EasyManagePwsh([string]$Hint,[bool]$Machine=$false) {
  function Test-MachinePwsh([string]$p) {
    if($p.Replace('/','\') -match '(?i)\\WindowsApps\\'){return $false}
    $trusted=@('S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    try {
      $part=[IO.Path]::GetFullPath($p)
      while($part) {
        $item=Get-Item -LiteralPath $part -Force
        if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){return $false}
        $acl=Get-Acl -LiteralPath $part
        if($acl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin $trusted){return $false}
        $mask=0xD0156
        if($part -eq [IO.Path]::GetPathRoot($part)){$mask=0xD0040}
        foreach($ace in $acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {
          if($ace.AccessControlType -eq 'Allow' -and !($ace.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -and ([int]$ace.FileSystemRights -band $mask) -and $ace.IdentityReference.Value -notin $trusted){return $false}
        }
        $next=[IO.Path]::GetDirectoryName($part);if($next -eq $part){break};$part=$next
      }
      return $true
    }catch{return $false}
  }
  $probe='if($PSVersionTable.PSVersion.Major -lt 7){exit 1};[Console]::Write((Get-Process -Id $PID).Path)'
  $seen=@{}
  function Try-Pwsh($paths) {
    foreach($p in $paths) {
      if(!$p -or $seen.ContainsKey($p)){continue};$seen[$p]=$true
      if(!(Test-Path -LiteralPath $p -PathType Leaf)){continue}
      if($Machine -and !(Test-MachinePwsh $p)){continue}
      $x=New-Object Diagnostics.Process
      try {
        $a=$x.StartInfo;$a.FileName=$p;$a.UseShellExecute=$false;$a.CreateNoWindow=$true
        $a.Arguments='-NoLogo -NoProfile -NonInteractive -Command "'+$probe+'"'
        $a.RedirectStandardInput=$true;$a.RedirectStandardOutput=$true;$a.RedirectStandardError=$true
        [void]$x.Start();$x.StandardInput.Close()
        if(!$x.WaitForExit(3000)){$x.Kill();$x.WaitForExit();continue}
        $v=$x.StandardOutput.ReadToEnd()
        if($x.ExitCode -eq 0 -and $v -and (Test-Path -LiteralPath $v -PathType Leaf) -and (!$Machine -or (Test-MachinePwsh $v))){return $v}
      }catch{}finally{$x.Dispose()}
    }
  }
  $paths=@();if($Hint -and [IO.Path]::IsPathRooted($Hint)){$paths+= $Hint}
  $paths+=@(Get-Command pwsh.exe -CommandType Application -All -ErrorAction SilentlyContinue | ForEach-Object {$_.Source})
  foreach($base in @($env:ProgramW6432,$env:ProgramFiles,${env:ProgramFiles(x86)},"$env:LOCALAPPDATA\Programs")) {
    if($base){$paths+=Join-Path $base 'PowerShell\7\pwsh.exe'}
  }
  if($env:LOCALAPPDATA){$paths+=Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\pwsh.exe'}
  $p=Try-Pwsh $paths;if($p){return $p}
  $paths=@()
  $keys=@('HKLM:\SOFTWARE\Microsoft\PowerShellCore\InstalledVersions\*')
  if(!$Machine){$keys+='HKCU:\SOFTWARE\Microsoft\PowerShellCore\InstalledVersions\*'}
  foreach($key in $keys) {
    Get-ItemProperty $key -ErrorAction SilentlyContinue | ForEach-Object {if($_.InstallLocation){$paths+=Join-Path $_.InstallLocation 'pwsh.exe'}}
  }
  $p=Try-Pwsh $paths;if($p){return $p}
  if($Machine){throw 'Machine-wide protected PowerShell 7+ required for LocalSystem'}
  $paths=@(Get-AppxPackage -Name Microsoft.PowerShell -ErrorAction SilentlyContinue | ForEach-Object {Join-Path $_.InstallLocation 'pwsh.exe'})
  $p=Try-Pwsh $paths;if($p){return $p}
  throw 'PowerShell 7+ not found for this account; install it or supply its absolute path'
}
