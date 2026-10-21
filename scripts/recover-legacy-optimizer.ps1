

#Requires -Version 5.1
[CmdletBinding(SupportsShouldProcess=$true, ConfirmImpact='High')]
param([switch]$Apply, [switch]$Elevated, [switch]$LibraryOnly)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
function Test-WrapperDebugger([string]$Command) {
    $path = $Command.Trim()
    $quoted = $path.StartsWith('"', [StringComparison]::Ordinal)
    if ($quoted) {
        if ($path.Length -lt 2 -or -not $path.EndsWith('"', [StringComparison]::Ordinal)) {
            return $false
        }
        $path = $path.Substring(1, $path.Length - 2)
    }
    $ordinaryPath = $path
    if ($path.StartsWith('\\?\', [StringComparison]::Ordinal) -or $path.StartsWith('\\.\', [StringComparison]::Ordinal)) {
        $ordinaryPath = $path.Substring(4)
    }
    $invalid = [char[]]@([char]34, [char]60, [char]62, [char]124, [char]42, [char]63, [char]0, [char]13, [char]10, [char]9)
    if ($ordinaryPath.IndexOfAny($invalid) -ge 0) {
        return $false
    }

    $separator = $path.LastIndexOfAny([char[]]@('\', '/'))
    $name = $path.Substring($separator + 1)
    if (-not $name.Equals('service.exe', [StringComparison]::OrdinalIgnoreCase) -and
        -not $name.Equals('wrapper.exe', [StringComparison]::OrdinalIgnoreCase)) {
        return $false
    }
    if ($separator -lt 0) {
        return $true
    }

    $colon = $ordinaryPath.IndexOf(':')
    if ($colon -ge 0) {
        if ($colon -ne 1 -or $ordinaryPath.Length -lt 3 -or $ordinaryPath.IndexOf(':', 2) -ge 0) {
            return $false
        }
        $drive = [int][char]$ordinaryPath[0]
        if (($drive -lt 65 -or $drive -gt 90) -and ($drive -lt 97 -or $drive -gt 122)) {
            return $false
        }
        if ($ordinaryPath[2] -ne '\' -and $ordinaryPath[2] -ne '/') {
            return $false
        }
    } elseif (-not $path.StartsWith('\\', [StringComparison]::Ordinal) -and
              -not $path.StartsWith('//', [StringComparison]::Ordinal)) {
        if (-not $path.StartsWith('%', [StringComparison]::Ordinal)) {
            return $false
        }
        $end = $path.IndexOf('%', 1)
        if ($end -le 1 -or $end + 1 -ge $path.Length -or ($path[$end + 1] -ne '\' -and $path[$end + 1] -ne '/')) {
            return $false
        }
    }

    
    
    if (-not $quoted) {
        for ($i = 0; $i -lt $path.Length; $i++) {
            if ($i + 4 -lt $path.Length -and $path.Substring($i, 4).Equals('.exe', [StringComparison]::OrdinalIgnoreCase) -and $path[$i + 4] -eq ' ') {
                return $false
            }
            if ($path[$i] -eq ' ' -and $i + 1 -lt $path.Length -and ($path[$i + 1] -eq '\' -or $path[$i + 1] -eq '/')) {
                return $false
            }
        }
    }
    return $true
}

function Remove-LegacyDebugger {
    param($Base, [string]$Target, [string]$Expected, $Kind)
    if ($Target -notin @('stalzone.exe','stalzonew.exe','stalcraft.exe','stalcraftw.exe')) { throw 'Unexpected target' }
    if (-not (Test-WrapperDebugger $Expected)) {throw 'Not a legacy wrapper path-only entry'}
    $path='SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\'+$Target
    $key=$Base.OpenSubKey($path,[Microsoft.Win32.RegistryKeyPermissionCheck]::Default,([Security.AccessControl.RegistryRights]::QueryValues -bor [Security.AccessControl.RegistryRights]::SetValue))
    if ($null -eq $key) {return}
    try {
        $current=$key.GetValue('Debugger',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        if ($null -eq $current) {return}
        if ($key.GetValueKind('Debugger') -ne $Kind -or -not [string]::Equals([string]$current,$Expected,[StringComparison]::Ordinal)) {throw 'Debugger changed after preview; left unchanged'}
        $key.DeleteValue('Debugger',$false)
    } finally {$key.Dispose()}
}
if ($LibraryOnly) {return}
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {throw 'Windows required'}

if ($Apply -and -not $WhatIfPreference) {
    $identity=[Security.Principal.WindowsIdentity]::GetCurrent()
    try {$admin=([Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)}finally{$identity.Dispose()}
    if (-not $admin) {
        if ($Elevated) {throw 'Administrator rights were not granted'}
        $hostPath=(Get-Process -Id $PID).Path
        $arguments=@('-NoProfile','-ExecutionPolicy','Bypass','-File',('"'+$PSCommandPath+'"'),'-Apply','-Elevated')
        $child=Start-Process -FilePath $hostPath -ArgumentList $arguments -Verb RunAs -Wait -PassThru
        exit $child.ExitCode
    }
}
$view=if ([Environment]::Is64BitOperatingSystem) {[Microsoft.Win32.RegistryView]::Registry64}else{[Microsoft.Win32.RegistryView]::Registry32}
$base=[Microsoft.Win32.RegistryKey]::OpenBaseKey([Microsoft.Win32.RegistryHive]::LocalMachine,$view)
try {
    $entries=@()
    foreach($target in @('stalzone.exe','stalzonew.exe','stalcraft.exe','stalcraftw.exe')) {
        $path='SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\'+$target
        $key=$base.OpenSubKey($path,$false)
        if ($null -eq $key) {continue}
        try {
            $command=$key.GetValue('Debugger',$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            if ($null -eq $command) {continue}
            $kind=$key.GetValueKind('Debugger')
            if ($kind -notin @([Microsoft.Win32.RegistryValueKind]::String,[Microsoft.Win32.RegistryValueKind]::ExpandString) -or -not (Test-WrapperDebugger ([string]$command))) {
                Write-Warning "$target : other Debugger entry; preserved."
                continue
            }
            $entries+= [pscustomobject]@{target=$target;command=[string]$command;kind=[string]$kind}
        } finally {$key.Dispose()}
    }
    if ($entries.Count -eq 0) {Write-Output 'No matching legacy entries found.';return}
    Write-Output 'Possible legacy wrapper entries. A filename alone does not prove which program owns them; review the complete paths.'
    $entries | Format-Table -AutoSize
    if (-not $Apply) {Write-Output 'Preview only. Run with -Apply to review/confirm removal. No values, files or permissions were changed.';return}
    $backup=$null
    foreach($entry in $entries) {
        if ($PSCmdlet.ShouldProcess(('HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\'+$entry.target+'\Debugger'),('Remove the exact entry: '+$entry.command))) {
            if ($null -eq $backup) {
                $backup=Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) ('ExboLauncherNew\recovery\IFEO-'+[guid]::NewGuid().ToString('N')+'.json')
                New-Item -ItemType Directory -Force -Path ([IO.Path]::GetDirectoryName($backup)) | Out-Null
                [IO.File]::WriteAllText($backup,($entries|ConvertTo-Json -Depth 4),[Text.UTF8Encoding]::new($false))
                Write-Output ('Debugger backup: '+$backup)
            }
            Remove-LegacyDebugger -Base $base -Target $entry.target -Expected $entry.command -Kind ([Enum]::Parse([Microsoft.Win32.RegistryValueKind],$entry.kind))
            Write-Output ($entry.target+': Debugger removed; key, ACL, other values and files preserved.')
        }
    }
} finally {$base.Dispose()}
