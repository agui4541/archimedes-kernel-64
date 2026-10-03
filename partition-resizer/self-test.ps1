<# Offline planner/CRC tests. No ADB and no device writes. #>
$ErrorActionPreference='Stop'
function Crc32([byte[]]$b){[UInt64]$c=[UInt64]4294967295;foreach($x in $b){$c=($c-bxor[UInt64]$x)-band[UInt64]4294967295;for($j=0;$j-lt8;$j++){if(($c-band1)-ne0){$c=(($c-shr1)-bxor[UInt64]3988292384)-band[UInt64]4294967295}else{$c=$c-shr1}}};$u=(($c-bxor[UInt64]4294967295)-band[UInt64]4294967295);if($u-ge2147483648){return [Int64]($u-4294967296)};return [Int64]$u}
function S([UInt64]$a,[UInt64]$b,[string]$n){[pscustomobject]@{Start=$a;End=$b;Name=$n;Number=([int]$script:n++)}}
function Plan($parts,[string]$target,[string]$donor,[UInt64]$size){
  $t=$parts|? Name -eq $target;$d=$parts|? Name -eq $donor;$old=$t.End-$t.Start+1;$nt=$size*2048;$between=$parts|?{$_.Start -gt $t.Start-and$_.Start -lt $d.Start}|sort Start;$new=@{};$new[$t.Number]=S $t.Start ($t.Start+$nt-1) $t.Name;$prev=$new[$t.Number].End;foreach($p in $between){$ns=[math]::Max($p.Start,$prev+1);$new[$p.Number]=S $ns ($ns+($p.End-$p.Start)) $p.Name;$prev=$new[$p.Number].End};$ds=$prev+1;if($ds -gt $d.End){throw 'donor exhausted'};return [pscustomobject]@{Parts=$new;DonorStart=$ds;DonorEnd=$d.End}
}
$crc=[Text.Encoding]::ASCII.GetBytes('123456789');if((Crc32 $crc)-ne [Int64]-873187034){throw 'CRC32 mismatch'}
foreach($case in @(@{sector=512;gap=0},@{sector=512;gap=4096},@{sector=4096;gap=0})){
  for($round=0;$round-lt 10;$round++){
    $script:n=1;$parts=@(S 2048 (2048+5242879) 'system';S (2048+5242880+$case.gap) ((2048+5242880+$case.gap)+29503) 'vbmeta';S ((2048+5242880+$case.gap)+29504) (((2048+5242880+$case.gap)+29504)+442367) 'cache';S (((2048+5242880+$case.gap)+29504)+442368) ((((2048+5242880+$case.gap)+29504)+442368)+4000000) 'userdata')
    $want=if(($round%2)-eq0){3072}else{2048};$p=Plan $parts 'system' 'userdata' $want;$s=$p.Parts[1].Start;if($s -le $p.Parts[0].End){throw 'overlap'};if($p.DonorStart -le $p.Parts[1].End){throw 'donor overlap'}
  }
}
'offline planner tests: PASS (CRC, 512/4K sectors, gaps, 10 expand/shrink cycles)'
