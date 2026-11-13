param([Parameter(Mandatory)][string]$Root,[Parameter(Mandatory)][string]$Pwsh)
$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$Root=[IO.Path]::GetFullPath($Root)
$temp=[IO.Path]::GetFullPath([IO.Path]::GetTempPath())
if(!$Root.StartsWith($temp,[StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($Root) -notmatch '^EasyManageReadonly-[a-f0-9]{32}$'){throw 'Refusing non-validation directory'}
if((Get-Item -LiteralPath $Root).Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Refusing linked validation directory'}
$fixture=Join-Path $Root ('scan-fixture-'+[guid]::NewGuid().ToString('N'))
$outside=Join-Path $Root ('outside-fixture-'+[guid]::NewGuid().ToString('N'))
if((Test-Path -LiteralPath $fixture) -or (Test-Path -LiteralPath $outside)){throw 'Fixtures must be new'}
[void][IO.Directory]::CreateDirectory($fixture);[void][IO.Directory]::CreateDirectory($outside)
$child=Join-Path $fixture 'unicode-子目录';[void][IO.Directory]::CreateDirectory($child)
[IO.File]::WriteAllBytes((Join-Path $fixture 'one.bin'),[byte[]]::new(1024))
[IO.File]::WriteAllBytes((Join-Path $child 'two.bin'),[byte[]]::new(2048))
[IO.File]::WriteAllBytes((Join-Path $outside 'not-in-scan.bin'),[byte[]]::new(65536))
New-Item -ItemType HardLink -Path (Join-Path $fixture 'duplicate.bin') -Target (Join-Path $fixture 'one.bin')|Out-Null
New-Item -ItemType Junction -Path (Join-Path $fixture 'excluded-junction') -Target $outside|Out-Null
function Invoke-Scanner([string]$Name,[hashtable]$Request,[bool]$Cancel=$false){
 $requestJson=($Request|ConvertTo-Json -Compress).Replace("'","''")
 $sourcePath=(Join-Path $Root 'scan-disk-space.ps1').Replace("'","''")
 $code="`$ProgressPreference='SilentlyContinue';`$request='$requestJson'|ConvertFrom-Json;& ([scriptblock]::Create([IO.File]::ReadAllText('$sourcePath')))"
 $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($code))
 $info=[Diagnostics.ProcessStartInfo]::new();$info.FileName=$Pwsh;$info.UseShellExecute=$false;$info.CreateNoWindow=$true
 $info.RedirectStandardInput=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 foreach($argument in @('-NoLogo','-NoProfile','-NonInteractive','-EncodedCommand',$encoded)){$info.ArgumentList.Add($argument)}
 $process=[Diagnostics.Process]::Start($info)
 $stdout=$process.StandardOutput.ReadToEndAsync();$stderr=$process.StandardError.ReadToEndAsync()
 if($Cancel){$process.StandardInput.Write('x');$process.StandardInput.Flush()}
 if(!$process.WaitForExit(20000)){$process.Kill($true);throw ('Only scan helper timed out: '+$Name)}
 $text=$stdout.GetAwaiter().GetResult();$errorText=$stderr.GetAwaiter().GetResult()
 [IO.File]::WriteAllText((Join-Path $Root ($Name+'.jsonl')),$text,[Text.UTF8Encoding]::new($false))
 [IO.File]::WriteAllText((Join-Path $Root ($Name+'.stderr.txt')),$errorText,[Text.UTF8Encoding]::new($false))
 $frames=@($text -split '\r?\n'|Where-Object{$_}|ForEach-Object{$_|ConvertFrom-Json})
 return @{exit=$process.ExitCode;frames=$frames;raw=$text;stderr=$errorText}
}
$path=$fixture.Replace('\','/')
$request=@{path=$path;scope='directory';mode='full';seconds=5;max_entries=1000}
$normal=Invoke-Scanner 'scan-normal' $request
if($normal.exit -ne 0){Write-Output $normal.stderr;throw 'Normal metadata scan failed'}
$result=@($normal.frames|Where-Object type -EQ result)[-1]
if(!$result -or $result.saved_directories -ne 2 -or $result.nodes[0].files -ne 3 -or $result.hardlinks -ne 1 -or $result.symlinks -ne 1){throw ('Unexpected scan summary: '+($result|ConvertTo-Json -Depth 8 -Compress))}
if($normal.raw -match 'one\.bin|two\.bin|duplicate\.bin|not-in-scan\.bin'){throw 'File names leaked into directory-only output'}
$entries=@($normal.frames|Where-Object type -EQ entries|ForEach-Object{$_.nodes})
if(@($entries|Where-Object kind -NE directory).Count){throw 'Non-directory entry emitted'}
if(@($entries|Where-Object{$_.id -eq 1 -and $_.name -eq 'unicode-子目录'}).Count -lt 1){throw 'Unicode directory name did not round-trip'}
if($result.nodes[0].apparent -lt 3072 -or $result.nodes[0].apparent -ge 65536){throw 'Hardlink or excluded-junction contents counted incorrectly'}
$limitedRequest=$request.Clone();$limitedRequest.max_entries=1
$limited=Invoke-Scanner 'scan-limited' $limitedRequest
$limitedResult=@($limited.frames|Where-Object type -EQ result)[-1]
if($limited.exit -ne 0 -or $limitedResult.reason -ne 'entry_limit' -or $limitedResult.saved_directories -ne 1){throw 'Entry budget failed'}
$cancelled=Invoke-Scanner 'scan-cancelled' $request $true
$cancelledResult=@($cancelled.frames|Where-Object type -EQ result)[-1]
if($cancelled.exit -ne 0 -or $cancelledResult.status -ne 'cancelled'){throw 'Cancellation failed'}
$wrongMode=$request.Clone();$wrongMode.mode='basic'
$denied=Invoke-Scanner 'scan-denied-mode' $wrongMode
if($denied.exit -eq 0){throw 'Administrator token accepted as Basic'}
$linked=$request.Clone();$linked.path=(Join-Path $fixture 'excluded-junction').Replace('\','/')
$deniedLink=Invoke-Scanner 'scan-denied-link' $linked
if($deniedLink.exit -eq 0){throw 'Junction root accepted'}
@{checks=8;normal=@{status=$result.status;files=$result.nodes[0].files;directories=$result.saved_directories;hardlinks=$result.hardlinks;reparse_points=$result.symlinks;apparent=$result.nodes[0].apparent};entry_budget=$limitedResult.reason;cancellation=$cancelledResult.status;privilege_mismatch_rejected=($denied.exit -ne 0);junction_root_rejected=($deniedLink.exit -ne 0);business_paths_scanned=$false}|ConvertTo-Json -Depth 6 -Compress
