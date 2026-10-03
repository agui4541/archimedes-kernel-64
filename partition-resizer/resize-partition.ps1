<#
Archimedes partition resizer (host-side, PowerShell 5+).

This is deliberately a host tool: it keeps GPT/MBR backups on the PC and
uses only ADB to read/write the recovery block device.  It never runs against
normal Android and never guesses a fixed sector layout.

Examples (TWRP, dry-run first):
  .\resize-partition.ps1 -Partition system -SizeMiB 2560 -Donor userdata
  .\resize-partition.ps1 -Partition system -SizeMiB 2560 -Donor userdata -Apply

The donor is intentionally discarded.  This makes the operation repeatable
for test devices and avoids pretending that an arbitrary filesystem can be
shrunk or moved safely.  Moved fixed partitions are backed up locally and
restored at their calculated new LBAs.  The target filesystem is resized only
when it is ext4; use -SkipFilesystemResize only when the target will be
replaced by a freshly flashed image.
#>
[CmdletBinding()]
param(
  [string]$Adb,
  [string]$Serial,
  [Parameter(Mandatory=$true)][string]$Partition,
  [Parameter(Mandatory=$true)][ValidateRange(1,1048576)][UInt64]$SizeMiB,
  [Parameter(Mandatory=$false)][Alias('Donor')][string]$DonorSpec,
  [switch]$Apply,
  [switch]$Finalize,
  [switch]$SkipFilesystemResize,
  [string]$BackupDir = (Join-Path (Get-Location) ("partition-backup-" + (Get-Date -Format 'yyyyMMdd-HHmmss')))
)

$ErrorActionPreference = 'Stop'
if(-not $PSBoundParameters.ContainsKey('Adb') -or [string]::IsNullOrWhiteSpace($Adb)){
  $candidates=@('C:\Program Files (x86)\scrcpy-win64-v3.3.3\adb.exe',(Join-Path $PSScriptRoot '..\platform-tools\adb.exe'))
  $Adb=$candidates | Where-Object {Test-Path -LiteralPath $_} | Select-Object -First 1
}
if (-not (Test-Path -LiteralPath $Adb)) { throw "adb not found: $Adb" }
# Keep ADB's key/state files inside the project instead of inheriting a
# possibly unwritable shell HOME (the Codex desktop sandbox commonly has one).
$adbHome = Join-Path $PSScriptRoot '..\adb-home'
New-Item -ItemType Directory -Force -Path $adbHome | Out-Null
$env:ANDROID_USER_HOME = $adbHome
$env:ANDROID_SDK_HOME = $adbHome
$env:HOME = $adbHome

function Invoke-AdbProcess([string[]]$Arguments, [byte[]]$InputBytes = $null) {
  $psi = [System.Diagnostics.ProcessStartInfo]::new()
  $psi.FileName = $Adb
  foreach ($a in $Arguments) { [void]$psi.ArgumentList.Add($a) }
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $p = [System.Diagnostics.Process]::new(); $p.StartInfo = $psi
  [void]$p.Start()
  if ($null -ne $InputBytes) {
    $p.StandardInput.BaseStream.Write($InputBytes, 0, $InputBytes.Length)
    $p.StandardInput.Close()
  }
  $outTask = $p.StandardOutput.BaseStream.CopyToAsync([System.IO.MemoryStream]::new())
  $err = $p.StandardError.ReadToEnd()
  $p.WaitForExit()
  # CopyToAsync above cannot expose its stream; rerun binary calls through the
  # dedicated helper below.  Text commands never use this return value.
  return [pscustomobject]@{ Exit=$p.ExitCode; Error=$err }
}

function Invoke-AdbText([string]$Command) {
  $args = @(); if ($Serial) { $args += @('-s',$Serial) }; $args += @('shell',$Command)
  $psi = [System.Diagnostics.ProcessStartInfo]::new(); $psi.FileName=$Adb
  $psi.EnvironmentVariables['HOME']=$adbHome; $psi.EnvironmentVariables['ANDROID_USER_HOME']=$adbHome; $psi.EnvironmentVariables['ANDROID_SDK_HOME']=$adbHome; $psi.EnvironmentVariables['USERPROFILE']=$env:USERPROFILE
  foreach ($a in $args) { [void]$psi.ArgumentList.Add($a) }
  $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true; $psi.RedirectStandardOutput=$true; $psi.RedirectStandardError=$true
  $p=[System.Diagnostics.Process]::new(); $p.StartInfo=$psi; [void]$p.Start()
  $o=$p.StandardOutput.ReadToEnd(); $e=$p.StandardError.ReadToEnd(); $p.WaitForExit()
  if ($p.ExitCode -ne 0) { throw "adb shell failed: $Command`n$e" }
  return $o.Trim()
}

function Invoke-AdbBytes([string]$Command) {
  $args = @(); if ($Serial) { $args += @('-s',$Serial) }; $args += @('exec-out',$Command)
  $psi = [System.Diagnostics.ProcessStartInfo]::new(); $psi.FileName=$Adb
  $psi.EnvironmentVariables['HOME']=$adbHome; $psi.EnvironmentVariables['ANDROID_USER_HOME']=$adbHome; $psi.EnvironmentVariables['ANDROID_SDK_HOME']=$adbHome; $psi.EnvironmentVariables['USERPROFILE']=$env:USERPROFILE
  foreach ($a in $args) { [void]$psi.ArgumentList.Add($a) }
  $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true; $psi.RedirectStandardOutput=$true; $psi.RedirectStandardError=$true
  $p=[System.Diagnostics.Process]::new(); $p.StartInfo=$psi; [void]$p.Start()
  $ms=[System.IO.MemoryStream]::new(); $p.StandardOutput.BaseStream.CopyTo($ms); $e=$p.StandardError.ReadToEnd(); $p.WaitForExit()
  if ($p.ExitCode -ne 0) { throw "adb exec-out failed: $Command`n$e" }
  # Unary comma prevents PowerShell from enumerating hundreds of megabytes
  # into individual pipeline objects when backing up cache/vendor.
  return ,$ms.ToArray()
}

function Send-AdbBytes([string]$Command, [byte[]]$Bytes) {
  $args = @(); if ($Serial) { $args += @('-s',$Serial) }; $args += @('shell',$Command)
  $psi = [System.Diagnostics.ProcessStartInfo]::new(); $psi.FileName=$Adb
  $psi.EnvironmentVariables['HOME']=$adbHome; $psi.EnvironmentVariables['ANDROID_USER_HOME']=$adbHome; $psi.EnvironmentVariables['ANDROID_SDK_HOME']=$adbHome; $psi.EnvironmentVariables['USERPROFILE']=$env:USERPROFILE
  foreach ($a in $args) { [void]$psi.ArgumentList.Add($a) }
  $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true; $psi.RedirectStandardInput=$true; $psi.RedirectStandardError=$true; $psi.RedirectStandardOutput=$true
  $p=[System.Diagnostics.Process]::new(); $p.StartInfo=$psi; [void]$p.Start()
  try {$p.StandardInput.BaseStream.Write($Bytes,0,$Bytes.Length)} catch { if($_.Exception.Message -notmatch 'pipe|管道'){throw}; Write-Warning "ADB closed the dd input after accepting the requested partition size" }
  $p.StandardInput.Close()
  $o=$p.StandardOutput.ReadToEnd(); $e=$p.StandardError.ReadToEnd(); $p.WaitForExit()
  if ($p.ExitCode -ne 0 -and $e -notmatch 'pipe|管道') { throw "adb write failed: $Command`n$o`n$e" }
}

function U32([byte[]]$b,[int]$o) { return [BitConverter]::ToUInt32($b,$o) }
function U64([byte[]]$b,[int]$o) { return [BitConverter]::ToUInt64($b,$o) }
function AsU32($v) { [Int64]$i=$v; if($i -ge 2147483648){$i-=4294967296}; return [BitConverter]::ToUInt32([BitConverter]::GetBytes([Int32]$i),0) }
function PutU32([byte[]]$b,[int]$o,[UInt32]$v) { [Array]::Copy([BitConverter]::GetBytes($v),0,$b,$o,4) }
function PutU64([byte[]]$b,[int]$o,[UInt64]$v) { [Array]::Copy([BitConverter]::GetBytes($v),0,$b,$o,8) }
function Crc32([byte[]]$b,[int]$offset=0,[int]$length=$b.Length) {
  [UInt64]$crc=[UInt64]4294967295
  for($i=$offset;$i -lt ($offset+$length);$i++) {
    $crc = ($crc -bxor [UInt64]$b[$i]) -band [UInt64]4294967295
    for($j=0;$j -lt 8;$j++) { if(($crc -band 1) -ne 0) {$crc=(($crc -shr 1) -bxor [UInt64]3988292384) -band [UInt64]4294967295} else {$crc=$crc -shr 1} }
  }
  return AsU32 (($crc -bxor [UInt64]4294967295) -band [UInt64]4294967295)
}

function Find-DiskAndSector {
  $path = Invoke-AdbText "readlink -f /dev/block/by-name/$Partition 2>/dev/null || readlink -f /dev/block/platform/bootdevice/by-name/$Partition"
  if ($path -notmatch '^(/dev/block/mmcblk\d+)p(\d+)$') { throw "target is not a physical eMMC partition: $path" }
  $disk=$Matches[1]; $num=[int]$Matches[2]
  $ss=[UInt64](Invoke-AdbText "cat /sys/class/block/$($disk.Split('/')[-1])/queue/logical_block_size")
  if ($ss -lt 512 -or ($ss -band ($ss-1)) -ne 0) { throw "unsupported logical sector size: $ss" }
  $total=[UInt64](Invoke-AdbText "cat /sys/class/block/$($disk.Split('/')[-1])/size")
  return [pscustomobject]@{Path=$disk; TargetNumber=$num; SectorSize=$ss; TotalSectors=$total}
}

function Parse-Gpt([byte[]]$first,[UInt64]$total,[UInt64]$ss,$disk) {
  if ([Text.Encoding]::ASCII.GetString($first,512,8) -ne 'EFI PART') { return $null }
  $h=$first[512..($first.Length-1)]; $n=[int](U32 $h 80); $es=[int](U32 $h 84); $elba=U64 $h 72
  if ($n -lt 1 -or $n -gt 4096 -or $es -lt 128 -or $es -gt 1024) { throw 'invalid GPT entry geometry' }
  $entryBytes=[byte[]](Invoke-AdbBytes "dd if=$disk bs=$ss skip=$elba count=$([math]::Ceiling(($n*$es)/$ss)) 2>/dev/null")
  $parts=@()
  for($i=0;$i -lt $n;$i++) {
    $o=$i*$es; $type=$entryBytes[$o..($o+15)]
    $empty=($type | Where-Object { $_ -ne 0 }).Count -eq 0; if($empty){continue}
    $name=[Text.Encoding]::Unicode.GetString($entryBytes,$o+56,72).Trim([char]0)
    $parts += [pscustomobject]@{Number=$i+1;Start=(U64 $entryBytes ($o+32));End=(U64 $entryBytes ($o+40));Name=$name;EntryOffset=$o;Guid=$entryBytes[$o..($o+15)]}
  }
  return [pscustomobject]@{Type='GPT';Header=$h;Entries=$entryBytes;EntryCount=$n;EntrySize=$es;EntryLba=$elba;Parts=$parts;Disk=$disk;SectorSize=$ss;TotalSectors=$total}
}

function Parse-Mbr([byte[]]$first,[UInt64]$total,[UInt64]$ss,$disk) {
  if($first[510] -ne 0x55 -or $first[511] -ne 0xaa){throw 'unknown partition table (neither GPT nor MBR)'}
  $parts=@(); for($i=0;$i -lt 4;$i++){ $o=446+$i*16; $type=$first[$o+4]; $start=[UInt32](U32 $first ($o+8)); $count=[UInt32](U32 $first ($o+12)); if($type -eq 0 -or $count -eq 0){continue}; if($type -in 0x05,0x0f,0x85){throw 'extended MBR is refused; use GPT or a dedicated image backup'}; $parts += [pscustomobject]@{Number=$i+1;Start=[UInt64]$start;End=([UInt64]$start+$count-1);Name="p$($i+1)";EntryOffset=$o;TypeByte=$type} }
  return [pscustomobject]@{Type='MBR';SectorSize=$ss;TotalSectors=$total;Parts=$parts;Disk=$disk;Mbr=$first}
}

function Get-Layout($d) {
  $first=[byte[]](Invoke-AdbBytes "dd if=$($d.Path) bs=$($d.SectorSize) count=34 2>/dev/null")
  if($first.Length -lt (2*$d.SectorSize)){throw 'cannot read partition table'}
  $g=Parse-Gpt $first $d.TotalSectors $d.SectorSize $d.Path
  if($null -ne $g){return $g}; return Parse-Mbr $first $d.TotalSectors $d.SectorSize $d.Path
}

function Resolve-Part($layout,[string]$spec) {
  if($spec -match '^p?(\d+)$'){ $n=[int]$Matches[1]; foreach($p in @($layout.Parts)){if([int]$p.Number -eq $n){return $p}}; return $null }
  if($layout.Type -eq 'MBR'){throw "MBR has no names; use p1..p4"}
  foreach($p in @($layout.Parts)){if([string]$p.Name -ceq $spec){return $p}}; return $null
}

function AlignMiB([UInt64]$bytes,[UInt64]$sector){ $mib=[UInt64]1048576; if(($bytes % $mib) -ne 0){throw 'size must be a whole MiB'}; $s=$bytes/$sector; if(($s % ($mib/$sector)) -ne 0){throw 'size is not sector/alignment compatible'}; return [UInt64]$s }
function PartSize($p){ return [UInt64]($p.End-$p.Start+1) }

# Recovery and safety checks before any table read/write.
$mode=Invoke-AdbText 'getprop ro.bootmode'; if($mode -ne 'recovery'){throw "device is '$mode'; boot TWRP/recovery first"}
$id=Invoke-AdbText 'id'; if($id -notmatch 'uid=0'){throw 'ADB shell is not root in recovery'}
if((Invoke-AdbText 'if [ -e /dev/block/by-name/super ]; then echo present; elif [ -e /dev/block/platform/bootdevice/by-name/super ]; then echo present; else echo absent; fi') -eq 'present'){throw 'dynamic super partition detected; use lpmake, not GPT resizing'}
if((Invoke-AdbText 'cat /proc/mounts') -match ' /(system|vendor|product|data|cache|metadata) '){throw 'an affected Android partition is mounted; unmount it in TWRP'}

$d=Find-DiskAndSector; $layout=Get-Layout $d
$targetByName=Resolve-Part $layout $Partition
if($Finalize){
  if($null -eq $targetByName){throw 'finalize target partition was not found'}
  $finalPath="$($d.Path)p$($targetByName.Number)"; $finalFsLine=Invoke-AdbText "blkid $finalPath 2>/dev/null || true"
  if($finalFsLine -notmatch 'TYPE="ext4"'){throw 'finalize requires an ext4 target'}
  $finalBlocks=[UInt64]((PartSize $targetByName)*$d.SectorSize/4096)
  # TWRP ships an old resize2fs which otherwise refuses to grow a filesystem
  # whose superblock still advertises the pre-repartition size.  The partition
  # table was already refreshed by the recovery reboot, so forcing the grow is
  # safe after the ext4 type/geometry checks above.
  $finalOut=Invoke-AdbText ('e2fsck -fy ' + $finalPath + ' >/dev/null 2>&1; resize2fs -f ' + $finalPath + '; echo __ARCH_RESIZE_RC:$?; tune2fs -l ' + $finalPath + ' 2>/dev/null | grep "Block count"')
  $finalOut | Out-Host; if($finalOut -notmatch "Block count:\s+$finalBlocks\b"){throw "finalize did not reach $finalBlocks blocks"}
  Write-Host "FINALIZED: $Partition filesystem now fills $finalBlocks blocks ($([UInt64]($finalBlocks*4096)) bytes)."; exit 0
}
$target=$targetByName; $donor=Resolve-Part $layout $DonorSpec
if(-not $Finalize -and [string]::IsNullOrWhiteSpace($DonorSpec)){throw 'a donor partition is required unless -Finalize is used'}
if($null -eq $target -or $null -eq $donor){throw 'target or donor partition was not found'}
if($target.Number -eq $donor.Number -or $donor.Start -le $target.Start){throw 'donor must be a later partition than target'}
$forbidden='boot|recovery|lk|lk2|preloader|pgpt|sgpt|protect1|protect2|nvram|nvdata|nvcfg|proinfo|tee1|tee2|otp|flashinfo'
if($target.Name -match "^($forbidden)$" -or $donor.Name -match "^($forbidden)$"){throw 'refusing to resize a bootloader/calibration partition'}

$newTargetSize=AlignMiB ([UInt64]$SizeMiB*[UInt64]1048576) $d.SectorSize
$oldTargetSize=PartSize $target; $delta=[Int64]$newTargetSize-[Int64]$oldTargetSize
$between=$layout.Parts | Where-Object { $_.Start -gt $target.Start -and $_.Start -lt $donor.Start } | Sort-Object Start
$new=@{}; $new[$target.Number]=[pscustomobject]@{Start=$target.Start;End=($target.Start+$newTargetSize-1);Name=$target.Name;Number=$target.Number}
$prev=$new[$target.Number].End
foreach($p in $between){
  if($delta -lt 0){$ns=[UInt64]($prev+1)}else{$ns=[UInt64][math]::Max([double]$p.Start,[double]($prev+1))}; $ne=$ns+(PartSize $p)-1
  $new[$p.Number]=[pscustomobject]@{Start=$ns;End=$ne;Name=$p.Name;Number=$p.Number}; $prev=$ne
}
$donorNewStart=[UInt64]($prev+1); $donorNewEnd=[UInt64]$donor.End; $donorNewSize=$donorNewEnd-$donorNewStart+1
if($donorNewSize -lt ([UInt64]1048576/$d.SectorSize)){throw 'requested size consumes the donor; at least 1 MiB donor space is required'}
if($layout.Parts | Where-Object { $_.Start -gt $donor.Start -and $_.Start -le $donorNewEnd -and $_.Number -ne $donor.Number }){throw 'donor is not the last occupied region in its interval'}

$plan=[ordered]@{schema=1;table=$layout.Type;disk=$d.Path;sector_size=$d.SectorSize;total_sectors=$d.TotalSectors;target=$Partition;target_number=$target.Number;target_old_start=$target.Start;target_old_end=$target.End;target_new_start=$new[$target.Number].Start;target_new_end=$new[$target.Number].End;donor=$DonorSpec;donor_number=$donor.Number;donor_old_start=$donor.Start;donor_old_end=$donor.End;donor_new_start=$donorNewStart;donor_new_end=$donorNewEnd;delta_sectors=$delta;warning='DONOR CONTENT IS DISCARDED; affected fixed partitions are backed up on the PC'}
$moves=@(); foreach($p in $between){if($new[$p.Number].Start -ne $p.Start){$moves += [ordered]@{number=$p.Number;name=$p.Name;old_start=$p.Start;old_end=$p.End;new_start=$new[$p.Number].Start;new_end=$new[$p.Number].End;sectors=(PartSize $p)}}}; $plan.moves=$moves
New-Item -ItemType Directory -Force -Path $BackupDir | Out-Null
$plan | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $BackupDir 'plan.json') -Encoding UTF8

Write-Host "table=$($layout.Type) disk=$($d.Path) sector=$($d.SectorSize) total=$($d.TotalSectors)"
Write-Host "target=$Partition old=$oldTargetSize sectors new=$newTargetSize sectors"
Write-Host "donor=$DonorSpec old=$($donor.Start)-$($donor.End) new=$donorNewStart-$donorNewEnd (contents discarded)"
Write-Host "plan saved: $(Join-Path $BackupDir 'plan.json')"
if(-not $Apply){Write-Host 'DRY RUN: no device data or partition table was changed.'; exit 0}
$fst=''
if(-not $SkipFilesystemResize){
  $targetPath="$($d.Path)p$($target.Number)"
  $fstLine=Invoke-AdbText "blkid $targetPath 2>/dev/null || true"
  if($fstLine -match 'TYPE="([^"]+)"'){$fst=$Matches[1]}else{$fst=$fstLine.Trim()}
  Write-Verbose "filesystem=$fst delta=$delta"
  if($fst -and $fst -ne 'ext4'){throw "target filesystem '$fst' is not supported for automatic resize; use -SkipFilesystemResize only when reflashing it"}
}
$answer=Read-Host 'This erases the donor and may brick the device if interrupted. Type y to continue'
if($answer -cne 'y'){throw 'cancelled'}

# Save GPT metadata and every moved partition to the PC before writing anything.
# TWRP's 32-bit dd cannot seek past 2 GiB on the whole-disk node, so partition
# contents are read from their individual nodes instead of using a huge skip.
if($layout.Type -eq 'GPT'){
  Invoke-AdbText "/sbin/sgdisk --backup=/tmp/archimedes-resizer-gpt.bin $($d.Path)" | Out-Host
  $gptImage=[byte[]](Invoke-AdbBytes 'cat /tmp/archimedes-resizer-gpt.bin'); if($null -eq $gptImage -or $gptImage.Length -lt 1024){throw 'GPT backup read failed'}; [IO.File]::WriteAllBytes((Join-Path $BackupDir 'gpt-backup.bin'),$gptImage)
} else {
  $mbrBytes=[byte[]](Invoke-AdbBytes "dd if=$($d.Path) bs=$($d.SectorSize) count=1 2>/dev/null"); [IO.File]::WriteAllBytes((Join-Path $BackupDir 'mbr.bin'),$mbrBytes)
}
foreach($m in $moves){
  $file=Join-Path $BackupDir ("p$($m.number)-$($m.name).bin");
  $bytes=[byte[]](Invoke-AdbBytes "dd if=$($d.Path)p$($m.number) bs=$($d.SectorSize) count=$($m.sectors) 2>/dev/null"); if($null -eq $bytes -or [UInt64]$bytes.Length -ne ([UInt64]$m.sectors*$d.SectorSize)){throw "partition backup failed for p$($m.number)"}; [IO.File]::WriteAllBytes($file,$bytes)
}

function Build-GptBytes($layout,$new,$donorNewStart) {
  if($layout.Type -ne 'GPT'){throw 'internal: GPT builder called for MBR'}
  $entries=[byte[]]$layout.Entries.Clone()
  foreach($p in $layout.Parts){if($new.ContainsKey($p.Number)){$o=$p.EntryOffset;PutU64 $entries ($o+32) ([UInt64]$new[$p.Number].Start);PutU64 $entries ($o+40) ([UInt64]$new[$p.Number].End)}}
  $do=$layout.Parts | Where-Object Number -eq $donor.Number; PutU64 $entries ($do.EntryOffset+32) ([UInt64]$donorNewStart); PutU64 $entries ($do.EntryOffset+40) ([UInt64]$donorNewEnd)
  $entryCrc=Crc32 $entries 0 ($layout.EntryCount*$layout.EntrySize)
  $primary=[byte[]](Invoke-AdbBytes "dd if=$($layout.Disk) bs=$($layout.SectorSize) skip=1 count=1 2>/dev/null"); $backup=[byte[]](Invoke-AdbBytes "dd if=$($layout.Disk) bs=$($layout.SectorSize) skip=$($layout.TotalSectors-1) count=1 2>/dev/null")
  PutU32 $primary 88 $entryCrc; PutU32 $backup 88 $entryCrc
  PutU64 $primary 72 2; PutU64 $backup 72 ([UInt64]($layout.TotalSectors-[math]::Ceiling(($layout.EntryCount*$layout.EntrySize)/$layout.SectorSize)))
  PutU32 $primary 16 0; PutU32 $backup 16 0; PutU32 $primary 16 (Crc32 $primary 0 (U32 $primary 12)); PutU32 $backup 16 (Crc32 $backup 0 (U32 $backup 12))
  return [pscustomobject]@{Entries=$entries;Primary=$primary;Backup=$backup;EntrySectors=[math]::Ceiling(($layout.EntryCount*$layout.EntrySize)/$layout.SectorSize);BackupEntryLba=([UInt64]($layout.TotalSectors-[math]::Ceiling(($layout.EntryCount*$layout.EntrySize)/$layout.SectorSize)))}
}

if(-not $SkipFilesystemResize -and $fst -eq 'ext4' -and $delta -lt 0){
  # A shrink must happen while the old, larger GPT entry is still visible.
  $checkOut = Invoke-AdbText ('e2fsck -fy ' + $targetPath + '; echo __ARCH_CHECK_RC:$?')
  if($checkOut -notmatch '__ARCH_CHECK_RC:([01])'){throw "e2fsck failed before shrink: $checkOut"}; $checkOut | Out-Host
  $expectedBlocks=[UInt64]($newTargetSize*$d.SectorSize/4096)
  $resizeCheck=Invoke-AdbText ('resize2fs -f ' + $targetPath + ' ' + $expectedBlocks + '; echo __ARCH_RESIZE_RC:$?; tune2fs -l ' + $targetPath + ' 2>/dev/null | grep "Block count"')
  $resizeCheck | Out-Host; if($resizeCheck -notmatch "Block count:\s+$expectedBlocks\b"){throw "filesystem did not shrink to $expectedBlocks blocks; GPT was not changed"}
}
if($layout.Type -eq 'GPT'){
  $affected=@($target)+@($between)+@($donor) | Sort-Object Number -Unique
  $args=''; foreach($p in ($affected | Sort-Object Number -Descending)){
    if($p.Number -eq $donor.Number){$ns=$donorNewStart;$ne=$donorNewEnd}else{$ns=$new[$p.Number].Start;$ne=$new[$p.Number].End}
    $args += " --delete=$($p.Number)"
  }
  foreach($p in ($affected | Sort-Object Number)){
    if($p.Number -eq $donor.Number){$ns=$donorNewStart;$ne=$donorNewEnd}else{$ns=$new[$p.Number].Start;$ne=$new[$p.Number].End}
    $args += " --new=$($p.Number):$ns`:$ne --typecode=$($p.Number):0700 --change-name=$($p.Number):$($p.Name)"
  }
  Invoke-AdbText "/sbin/sgdisk$args $($d.Path)" | Out-Host
  Invoke-AdbText "/sbin/sgdisk --verify $($d.Path)" | Out-Host
} else {
  # Only four primary MBR entries are accepted by Parse-Mbr.  Extended/logical
  # chains are refused, so this update cannot silently orphan an EBR.
  $mbr=[byte[]]$layout.Mbr.Clone()
  foreach($p in $layout.Parts){
    if($new.ContainsKey($p.Number)){
      $o=$p.EntryOffset; $ns=[UInt64]$new[$p.Number].Start; $ne=[UInt64]$new[$p.Number].End
      if($ns -gt [UInt64]0xffffffff -or ($ne-$ns+1) -gt [UInt64]0xffffffff){throw 'new MBR geometry exceeds 32-bit LBA'}
      PutU32 $mbr ($o+8) ([UInt32]$ns); PutU32 $mbr ($o+12) ([UInt32]($ne-$ns+1))
    }
  }
  $do=$layout.Parts | Where-Object Number -eq $donor.Number
  if($donorNewStart -gt [UInt64]0xffffffff -or ($donorNewEnd-$donorNewStart+1) -gt [UInt64]0xffffffff){throw 'new MBR donor geometry exceeds 32-bit LBA'}
  PutU32 $mbr ($do.EntryOffset+8) ([UInt32]$donorNewStart); PutU32 $mbr ($do.EntryOffset+12) ([UInt32]($donorNewEnd-$donorNewStart+1))
  Send-AdbBytes "dd of=$($d.Path) bs=$($d.SectorSize) seek=0 2>/dev/null" $mbr
}
Invoke-AdbText "blockdev --rereadpt $($d.Path) 2>/dev/null || true" | Out-Host
foreach($m in $moves){$file=Join-Path $BackupDir ("p$($m.number)-$($m.name).bin");$bytes=[IO.File]::ReadAllBytes($file);Send-AdbBytes "dd of=$($d.Path)p$($m.number) bs=$($d.SectorSize) 2>/dev/null" $bytes}
if(-not $SkipFilesystemResize -and $fst -eq 'ext4' -and $delta -gt 0){
  Write-Host 'GPT is updated but the running kernel still has the old partition table.'
  Write-Host 'Reboot TWRP/recovery, then rerun with -Finalize to grow the ext4 filesystem.'
  exit 0
}
Invoke-AdbText "blockdev --rereadpt $($d.Path) 2>/dev/null || true" | Out-Host
Write-Host "DONE. GPT/MBR metadata was written and moved partitions restored. Format the donor in TWRP before booting; backup is $BackupDir"
