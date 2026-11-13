$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
$out = @{}
$issues = [Collections.Generic.List[object]]::new()
function Section([string]$name, [scriptblock]$body) {
    try { $result = & $body; if($name -in @('users','proxy','diskio')) { $out[$name]=@($result) } else { $out[$name]=$result } } catch { $issues.Add(@{component=$name;message='Windows observation unavailable or access denied; cached timestamp retained'}) }
}
if ($request.identity) { Section 'identity' {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    @{platform='windows';principal=$id.User.Value;username=$id.Name;administrator=([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator);capabilities=@('metrics','users','software','network','permissions','terminal','service','disk_space')}
} }
if ($request.users) { Section 'users' {
    $groups = @{}
    $adminSids=[Collections.Generic.HashSet[string]]::new()
    foreach ($g in @(Get-LocalGroup)) {
        foreach ($m in @(Get-LocalGroupMember -Group $g.Name -ErrorAction Stop)) {
            if (!$groups.ContainsKey($m.SID.Value)) { $groups[$m.SID.Value]=[Collections.Generic.List[string]]::new() }
            $groups[$m.SID.Value].Add($g.Name)
            if($g.SID.Value -eq 'S-1-5-32-544'){[void]$adminSids.Add($m.SID.Value)}
        }
    }
    $sessionCounts=@{};$sessionsKnown=$true
    try {
        foreach($session in @(Get-CimInstance Win32_LogonSession -Filter 'LogonType = 2 OR LogonType = 10 OR LogonType = 11')) {
            foreach($account in @(Get-CimAssociatedInstance -InputObject $session -ResultClassName Win32_Account)) {if($account.SID){$sessionCounts[$account.SID]=1+[int]$sessionCounts[$account.SID]}}
        }
    }catch{$sessionsKnown=$false;$issues.Add(@{component='users_sessions';message='Windows interactive logon observations unavailable'})}
    $profiles=@{};foreach($profile in @(Get-CimInstance Win32_UserProfile)){$profiles[$profile.SID]=$profile.LocalPath}
    @((Get-LocalUser) | ForEach-Object {
        $sid=$_.SID.Value
        @{administrator=$adminSids.Contains($sid);native_id=$sid;username=$_.Name;uid=[uint32]::MaxValue;gid=[uint32]::MaxValue;home=[string]$profiles[$sid];shell='pwsh';groups=@($groups[$sid] | Where-Object {$_});sessions=$(if($sessionsKnown){[uint32]$sessionCounts[$sid]}else{$null});session_sources=@('windows_logon');session_evidence=$null;process_count=$null;locked=(!$_.Enabled);account_status=$(if($_.Enabled){'normal'}else{'restricted'})}
    })
} }
if ($request.network) { Section 'network' {
    $addresses=@(Get-NetIPAddress)
    @{interfaces=@(Get-NetAdapter -IncludeHidden | ForEach-Object {
        $adapter=$_
        @{name=$adapter.Name;kind=$(if($adapter.InterfaceDescription -match 'Tailscale'){'tailscale'}elseif($adapter.InterfaceDescription -match 'ZeroTier'){'zerotier'}else{switch([int]$adapter.InterfaceType){24{'loopback'};71{'wifi'};131{'tunnel'};default{'ethernet'}}});state=$adapter.Status.ToString().ToLowerInvariant();addresses=@($addresses|Where-Object InterfaceIndex -EQ $adapter.ifIndex|ForEach-Object{"$($_.IPAddress)/$($_.PrefixLength)"});mac_address=$adapter.MacAddress;mtu=$null;vlan_id=$null;master=$null;connection_name=$adapter.Name}
    });routes=@(Get-NetRoute | Where-Object {$_.State -ne 'Unreachable'} | Select-Object -First 2048 | ForEach-Object {@{destination=$_.DestinationPrefix;gateway=$_.NextHop;interface=$_.InterfaceAlias;metric=[uint32]$_.RouteMetric}})}
} }
if ($request.diskio) { Section 'diskio' {
 @(Get-CimInstance Win32_PerfRawData_PerfDisk_LogicalDisk | Where-Object Name -Match '^[A-Za-z]:$' | ForEach-Object {@{name=$_.Name;read=[uint64]$_.DiskReadBytesPersec;written=[uint64]$_.DiskWriteBytesPersec;timestamp=[uint64]$_.Timestamp_PerfTime;frequency=[uint64]$_.Frequency_PerfTime}})
} }
if ($request.storage) { Section 'storage' {
 @{block_devices=@(Get-Disk | ForEach-Object {
  $disk=$_
  $children=@(Get-Partition -DiskNumber $disk.Number | ForEach-Object {
   $partition=$_;$volume=$partition|Get-Volume -ErrorAction SilentlyContinue
   @{name="Disk $($disk.Number) partition $($partition.PartitionNumber)";path=$partition.AccessPaths|Select-Object -First 1;kind='partition';size_bytes=[uint64]$partition.Size;filesystem=[string]$volume.FileSystemType;label=$volume.FileSystemLabel;mount_points=@($partition.AccessPaths|Where-Object {$_ -match '^[A-Za-z]:\$'});model=$null;transport=$null;rotational=$null;removable=$null;read_only=$disk.IsReadOnly;partition_table=$null;partition_label=[string]$partition.Type;start_bytes=[uint64]$partition.Offset;children=@()}
  })
  @{name="Disk $($disk.Number)";path=$disk.Path;kind='disk';size_bytes=[uint64]$disk.Size;filesystem=$null;label=$null;mount_points=@();model=$disk.FriendlyName;transport=$disk.BusType.ToString();rotational=$null;removable=$null;read_only=$disk.IsReadOnly;partition_table=$disk.PartitionStyle.ToString();partition_label=$null;start_bytes=$null;children=$children}
 })}
} }
if ($request.software) { Section 'software' {
    # Registry metadata only: Win32_Product is deliberately never queried (MSI repair side effects).
    $paths=@('HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*','HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*','HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*')
    @{software_packages=@(Get-ItemProperty -Path $paths -ErrorAction SilentlyContinue|Where-Object DisplayName|Sort-Object DisplayName,DisplayVersion -Unique|Select-Object -First 10000|ForEach-Object{@{name=$_.DisplayName;version=[string]$_.DisplayVersion}});running_services=@(Get-Service | Where-Object Status -EQ Running | ForEach-Object{@{name=$_.Name;state='running'}})}
} }
if ($request.hardware) { Section 'hardware' {
    @{collected_at=[DateTime]::UtcNow.ToString('o');processors=@(Get-CimInstance Win32_Processor|ForEach-Object{$_.Name});motherboard=@(Get-CimInstance Win32_BaseBoard|ForEach-Object{"$($_.Manufacturer) $($_.Product)"});memory=@(Get-CimInstance Win32_PhysicalMemory|ForEach-Object{"$($_.Manufacturer) $($_.PartNumber) $($_.Capacity) bytes $($_.Speed) MHz"});graphics=@(Get-CimInstance Win32_VideoController|ForEach-Object{$_.Name});displays=@();disks=@(Get-CimInstance Win32_DiskDrive|ForEach-Object{"$($_.Model) $($_.Size) bytes"});audio=@(Get-CimInstance Win32_SoundDevice|ForEach-Object{$_.Name});network=@(Get-NetAdapter|ForEach-Object{$_.InterfaceDescription});notes=@('Only hardware metadata; serial numbers and machine secrets are not collected')}
} }
if ($request.permissions) { Section 'permissions' {
    $checks=[Collections.Generic.List[object]]::new()
    $base=$request.base
    if ($base -and (Test-Path -LiteralPath $base)) {
        $acl=Get-Acl -LiteralPath $base
        $allowed=@('S-1-5-18','S-1-5-32-544',[Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
        $broad=@($acl.Access|Where-Object { $_.AccessControlType -eq 'Allow' -and $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -notin $allowed -and ($_.FileSystemRights -band [Security.AccessControl.FileSystemRights]::Write) })
        $checks.Add(@{category='windows_acl';title='Agent directory ACL';path=$base;status=$(if($broad.Count){'warning'}else{'ok'});observed="Owner: $($acl.Owner); broad write rules: $($broad.Count)";expected='Only the service identity, SYSTEM and Administrators may write';detail='Read-only NTFS ACL inspection; no ACL changes'})
    }
    $profiles=@(Get-NetFirewallProfile)
    $checks.Add(@{category='windows_firewall';title='Windows Firewall';path='Windows Firewall profiles';status=$(if(@($profiles|Where-Object {!$_.Enabled}).Count){'warning'}else{'ok'});observed=($profiles|ForEach-Object{"$($_.Name)=$($_.Enabled)"}) -join '; ';expected='Firewall profiles enabled';detail='Read-only configured profile state; not a reachability or rule audit'})
    @{collected_at=[DateTime]::UtcNow.ToString('o');checks=$checks.ToArray();truncated=$false}
} }
if ($request.programming) { Section 'programming' {
    $programming=[Collections.Generic.List[object]]::new()
    foreach ($name in @('pwsh','python','python3','node','npm','git','cargo','rustc','go','java','dotnet','cmake','cl','conda','uv')) {
        $cmd=Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if($cmd){$programming.Add(@{name=$name;category='Windows PATH';executable_paths=@($cmd.Source);package_version=$null})}
    }
    @{programming_tools=$programming.ToArray()}
} }
if ($request.ai_tools) { Section 'ai_tools' {
    $ai=[Collections.Generic.List[object]]::new()
    foreach ($name in @('codex','claude','gemini','ollama','aider','opencode')) {
        $cmd=Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        $canReadOwners=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        $processes=if($canReadOwners){@(Get-Process -Name $name -IncludeUserName -ErrorAction SilentlyContinue)}else{@(Get-Process -Name $name -ErrorAction SilentlyContinue)}
        if($cmd -or $processes.Count){$ai.Add(@{name=$name;category='AI tools';executable_paths=@($cmd.Source|Where-Object {$_});processes=@($processes|ForEach-Object{@{pid=[uint32]$_.Id;uid=[uint32]::MaxValue;username=$(if($_.UserName){$_.UserName}else{'unknown'})}});service_units=@()})}
    }
    @{ai_tools=$ai.ToArray()}
} }
if ($request.proxy) { Section 'proxy' {
    $canReadOwners=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    $observedProcesses=if($canReadOwners){Get-Process -IncludeUserName}else{Get-Process}
    $processes=@($observedProcesses|Where-Object ProcessName -Match '^(mihomo|clash|clash-verge|sing-box|v2ray|xray)$')
    $listeners=@(Get-NetTCPConnection -State Listen)
    @($processes|ForEach-Object {
        $proc=$_
        @{name=$proc.ProcessName;pid=[uint32]$proc.Id;username=$(if($proc.UserName){$proc.UserName}else{'unknown'});uid=[uint32]::MaxValue;listen_addresses=@($listeners|Where-Object OwningProcess -EQ $proc.Id|ForEach-Object{"$($_.LocalAddress):$($_.LocalPort)"})}
    })
} }
$out.issues=$issues.ToArray()
ConvertTo-Json -InputObject $out -Depth 12 -Compress
