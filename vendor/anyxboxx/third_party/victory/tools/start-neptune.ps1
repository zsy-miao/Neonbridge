# Neptune test launcher (zm0kie): sets Neptune's switch set itself (from its play.ps1) instead of calling play.ps1,
# so the switches below can be changed. Own cache and data folders; D:\victory-fork and %LOCALAPPDATA%\victory stay untouched.
#   .\start-neptune.ps1 -Stats -NoFastCopy            RAM-saving profile (default)
#   .\start-neptune.ps1 -Stats -NoFastCopy -Original  Neptune's own values
#   -AllNew                       -WindowUpload -DropDxil
#   -Watch4K / -SlotEvict         (gave black water patches / crashed the game: leave off)
#   -Off ASK_QUIET,TEXTURE_NO_WAIT leave Neptune switches out, to find what darkens objects
#   -BackKinds 256x256f41         with -Ocean: copy back only these kinds (WxHfFORMAT, comma list)
#   -DescData                     40 more shaders translate (water wake/splash candidates)
#   -Rebar                        windows in ReBAR VRAM (experiment: may bring settles back)
#   -DsNotify                     DirectStorage writes reach the copies
#   -Ocean                        ocean height map read-back (boats in Gone Fishing)
#   -WindowUpload                 windows on the card keep no copy in the PC's memory (-Built only)
#   -WindowKB 256                 finer windows: less of the title's memory copied into the PC's memory
#   -TexRoom / -WindowsMB         card shares (TexRoom 75 made textures black: leave it)
#   -Built uses our own release build of D:\Neptune-VE\src (with the ram line)
#   -Stats -Tag A                 log and memory samples as statsA.log / memwatchA.csv (memwatch starts by itself)
#   -Diag uses the diagnostics build (memory line in the log)
#   -Warm first translates the shaders (already done once).
param([switch]$Warm, [switch]$Stats, [switch]$NoViewGuard, [switch]$NoFastCopy, [switch]$Original, [switch]$NoAutoA, [switch]$Diag, [switch]$Built, [int]$TexRoom = 0, [int]$WindowsMB = 0, [int]$WindowKB = 0, [switch]$WindowUpload, [string]$Tag = '', [switch]$DropDxil, [int]$ListIdle = 0, [int]$VramEvict = 0, [switch]$LooseKey, [switch]$AllNew, [switch]$Ocean, [switch]$Watch4K, [switch]$SlotEvict, [switch]$DsNotify, [string]$Off = '', [string]$BackKinds = '', [switch]$Rebar, [switch]$DescData)
Get-ChildItem Env:VICTORY_* -ErrorAction SilentlyContinue | Remove-Item
$set = [ordered]@{
  # Neptune's play.ps1, settled
  VICTORY_CARD_WINDOWS_MB     = '4096'
  VICTORY_BAKE_TEXTURES_MS    = '10000'
  VICTORY_EARLY_TEXTURES_MS   = '40'
  # VICTORY_WAVE_OWN_LDS_GROUPS = '64,128'  # Neptune limited the wave rule to these group sizes; unset = all sizes, which fixed the black foam
  VICTORY_WIDE_MERGE          = '1'
  VICTORY_RELAY_OWN_QUEUE     = '1'
  VICTORY_UNDER_WRITES_WORDS  = '1'
  VICTORY_UNDER_WRITES_RUNS   = '100000'
  VICTORY_UNDER_WRITES_MASKS  = '1'
  # Neptune's play.ps1, experimental
  VICTORY_RELAY               = '2'
  VICTORY_WIDE_REFRESH        = '1'
  VICTORY_ASK_ONCE            = '1'
  VICTORY_TEXTURE_NO_WAIT     = '1'
  VICTORY_CARD_REHASH_RUNS    = '1'
  VICTORY_JOIN_COPIES         = '1'
  VICTORY_D3D12_BACKGROUND    = '0'
  VICTORY_LATE_WAIT_LISTS     = '1024'
  VICTORY_WINDOW_TAILS        = '1'
  VICTORY_TEXTURE_STAGE_MB    = '512'
  VICTORY_CARD_BUFFERS_MIN_KB = '64'
  VICTORY_CARD_BUFFERS_MB     = '3072'
  VICTORY_ASK_QUIET           = '8'
  VICTORY_COPY_CHANGED        = '1'
  VICTORY_QUIET_BACK          = '1'
  VICTORY_LIST_POOL_MOST      = '2048'
  VICTORY_LIST_POOL_MB        = '2048'
  VICTORY_WIDE_THREADS        = '12'
}
if (-not $Original) {
  # RAM-saving profile for 32 GB and an 8-core CPU
  $set['VICTORY_WINDOW_KB']        = '1024'
  $set['VICTORY_LIST_POOL_MB']     = '512'
  $set['VICTORY_CARD_BUFFERS_MB']  = '1536'
  $set['VICTORY_TEXTURE_STAGE_MB'] = '256'
  $set['VICTORY_WIDE_THREADS']     = '6'
}
# VRAM: textures picked by index stop at this share of the card's budget (Neptune: 90), and windows on the card (MB)
if ($TexRoom -gt 0) { $set['VICTORY_TEXTURE_ROOM_PERCENT'] = "$TexRoom" }
if ($WindowKB -gt 0) { $set['VICTORY_WINDOW_KB'] = "$WindowKB" }   # grid of the title-memory windows (64 to 4096 KB; RAM profile 1024)
# Our RAM/VRAM options (-Built only). -AllNew = all of them with tested-looking values.
if ($AllNew) { $WindowUpload = $true; $DropDxil = $true }   # what measured useful (VramEvict/LooseKey/ListIdle did not help)
if ($WindowUpload) { $set['VICTORY_CARD_WINDOW_UPLOAD'] = '1' }
if ($Watch4K) { $set['VICTORY_WATCH_4K'] = '1' }                    # copy only the 4 KiB pages written, not whole 64 KiB
if ($SlotEvict) { $set['VICTORY_SLOT_EVICT'] = '1' }                # let go of idle textures that slots hold (VRAM)
if ($DsNotify) { $set['VICTORY_DS_NOTIFY'] = '1' }                  # DirectStorage writes through our own view reach the copies
if ($Rebar) { $set['VICTORY_WINDOW_REBAR'] = '1' }                 # windows as one copy in ReBAR VRAM (no twin, no send)
if ($DescData) { $set['VICTORY_DESC_DATA'] = '1' }                  # translate shaders that read descriptors as data (40 more, incl. water ones)
if ($Ocean) { $set['VICTORY_OCEAN_READBACK'] = '1' }
if ($BackKinds) { $set['VICTORY_TEXTURE_BACK'] = $BackKinds }   # which GPU-written textures go back, e.g. 256x256f41                 # ocean height map copied back for the boats' physics
if ($DropDxil) { $set['VICTORY_DROP_DXIL'] = '1' }                   # release translated shader code after pipeline creation
if ($ListIdle -gt 0) { $set['VICTORY_LIST_POOL_IDLE_S'] = "$ListIdle" }  # destroy host lists idle this many seconds
if ($VramEvict -gt 0) { $set['VICTORY_VRAM_EVICT'] = "$VramEvict" }   # evict least recently used textures above this % of the card's budget
if ($LooseKey) { $set['VICTORY_PLACED_LOOSE_KEY'] = '1' }           # one host copy per placed texture memory
if ($WindowsMB -gt 0) { $set['VICTORY_CARD_WINDOWS_MB'] = "$WindowsMB" }
if (-not $NoAutoA) { $set['VICTORY_AUTO_A'] = '150' }
# -Off "ASK_QUIET,TEXTURE_NO_WAIT": leave these of Neptune's switches out (names without VICTORY_)
foreach ($name in ($Off -split ',' | Where-Object { $_.Trim() })) { $set.Remove('VICTORY_' + $name.Trim().ToUpper()) }
foreach ($k in $set.Keys) { Set-Item -Path ("Env:" + $k) -Value $set[$k] }
$env:VICTORY_CACHE_DIR = 'D:\Neptune-VE\cache'
$env:VICTORY_DATA_DIR  = 'D:\Neptune-VE\data'
$env:VICTORY_DISCORD   = '0'
$env:VICTORY_GAME_DIR  = 'D:\ProjectAmerica'
if ($NoFastCopy) { $env:VICTORY_FAST_COPY = '0' }     # its plain copy dies on a guard page here (0x80000001 in texture::readable)
if ($NoViewGuard) { $env:VICTORY_VIEW_GUARD = '0' }
if (Get-Process victory -ErrorAction SilentlyContinue) { 'The game is already running.'; exit 1 }
$game = 'D:\ProjectAmerica\game_scarlett_master.exe'
Set-Location D:\Neptune-VE
$exe = if ($Diag) { 'D:\Neptune-VE\target\diag\victory.exe' } elseif ($Built) { 'D:\Neptune-VE\target\release\victory.exe' } else { '.\victory.exe' }   # -Diag: own diagnostics build of Neptune's source (memory line)
if ($Warm) { & $exe warm }
"profile: " + $(if ($Original) { 'Neptune original' } else { 'RAM-saving' }) + ", exe: $exe, textures to $($set['VICTORY_TEXTURE_ROOM_PERCENT'])%, windows on card $($set['VICTORY_CARD_WINDOWS_MB']) MB, window grid $($set['VICTORY_WINDOW_KB']) KB, window upload $($set['VICTORY_CARD_WINDOW_UPLOAD']), drop dxil $($set['VICTORY_DROP_DXIL']), list idle $($set['VICTORY_LIST_POOL_IDLE_S']), vram evict $($set['VICTORY_VRAM_EVICT']), loose key $($set['VICTORY_PLACED_LOOSE_KEY']), ocean $($set['VICTORY_OCEAN_READBACK']), watch 4k $($set['VICTORY_WATCH_4K']), slot evict $($set['VICTORY_SLOT_EVICT']), ds notify $($set['VICTORY_DS_NOTIFY']), fast copy $(if ($env:VICTORY_FAST_COPY -eq '0') {'off'} else {'on'}), off: $Off, back kinds: $($set['VICTORY_TEXTURE_BACK']), rebar $($set['VICTORY_WINDOW_REBAR']), desc data $($set['VICTORY_DESC_DATA'])"
if ($Stats) {
  # Log and memory samples under one name: stats<Tag>.log and memwatch<Tag>.csv (Tag defaults to the time).
  if (-not $Tag) { $Tag = Get-Date -Format 'MMdd-HHmm' }
  $log = "D:\Neptune-VE\stats$Tag.log"; $mem = "D:\Neptune-VE\memwatch$Tag.csv"
  Start-Process powershell -WindowStyle Hidden -ArgumentList @('-ExecutionPolicy','Bypass','-File','D:\Neptune-VE\memwatch.ps1','-Out',$mem)
  "log: $log, memory samples: $mem"
  $env:VICTORY_LOG = 'victory_gpu=info,warn'
  & $exe run $game --execute 2> $log
  Copy-Item $log D:\Neptune-VE\stats.log -ErrorAction SilentlyContinue
} else {
  & $exe run $game --execute
}
