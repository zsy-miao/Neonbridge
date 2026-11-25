# Temporary XG reference trace

This diagnostic forwards calls to a locally supplied XG DLL. It is a research
tool, not a runtime provider. The reference DLL must not be committed or
redistributed.

Prepare a new directory outside the repository:

```sh
python tools/reference/xg-proxy/prepare.py \
  --original-xg /path/to/xg_xs.dll --output /tmp/new-xg-reference-trace
```

For a whole scene, add `--unique` to capture the first 16,384 distinct
descriptor and HRESULT pairs. Each record keeps its original call number.

The directory contains `xg_xs.dll`, `xg_reference.dll`, a guest control and
`provenance.json` with hashes. Use `stage.py` with a separate existing Frosty
work directory prepared with TuxBox XG and `--no-select`. It verifies the staged
hashes, holds that work directory's `launch.lock`, records honest temporary
provider origins, and restores the exact prior XG DLL and manifest afterward.
It does not alter game settings or the selected default work directory.

The command after `--` must stay in the foreground until the title and all of
its child processes have exited; a detached launcher would restore the DLL too
early. The ordinary Frosty launcher cannot be used inside this helper because
it tries to acquire the same lock. For example, from a desktop session:

```sh
WINEDEBUG='-all,+timestamp,err+all,warn+debugstr,warn+seh' \
python tools/reference/xg-proxy/stage.py --work /path/to/separate-work \
  --proxy-dir /tmp/new-xg-reference-trace -- \
  python tuxbox.py run --backend proton --proton /path/to/Proton \
  --prefix /path/to/separate-work/prefix --mangohud \
  --log-file /tmp/xg-trace/tuxbox.log /path/to/separate-work/Mount
```

Search the log for `XG_REFERENCE_TRACE`. If the helper is killed abruptly or
the machine loses power, run `python tools/reference/xg-proxy/stage.py --work
/path/to/separate-work --recover` after all title processes have stopped. The
helper checks the backup and staged hashes before restoring; unexpected changes
require manual inspection. Never overwrite the installed title or its DLLs.

Only `XGCreateTextureComputer` (ordinal 11) is traced; the remaining exports
are PE forwarders. By default the proxy records non-null descriptors and their
HRESULTs during the first 16,384 calls. A limit warning marks the end of
capture. The proxy returns the reference object's
pointer and HRESULT without wrapping the object, and restores the reference
call's last-error value after logging. Its diagnostics add overhead and do not
demonstrate TuxBox XG support or graphics correctness.

For the independent guest control, copy a compatible `xmem.dll` into the new
directory and run `xg-proxy-control.exe` under Wine or Proton. The control
compares successful and failing calls with the reference, including layout,
last-error, release, and one forwarded export.
