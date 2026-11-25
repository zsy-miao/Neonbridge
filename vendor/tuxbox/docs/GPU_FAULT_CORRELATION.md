# Offline Linux GPU fault correlation

Use the existing launch diagnosis first to establish the launch interval and
whether a terminal event was actually recorded. The new tool correlates an
explicit, timezone-aware observation window with kernel records from the same
boot. It works with synthetic GPU tests as well as authorized application runs.

Capture from the relevant boot, with exact start and end times:

```sh
journalctl -k -b --since '2026-11-24 18:00:00 UTC' --until '2026-11-24 18:01:00 UTC' -o json --no-pager > gpu-journal.jsonl
python tools/correlate_gpu_journal.py gpu-journal.jsonl \
    --start 2026-11-24T18:00:00+00:00 \
    --end 2026-11-24T18:01:00+00:00 \
    --boot-id "$(tr -d '-' </proc/sys/kernel/random/boot_id)" \
    --json gpu-fault-report.json
```

Use the actual capture boot ID, not the current boot ID, when analysing an
older capture. The example timestamps are illustrative. If the launcher log
has no verified terminal timestamp, explicitly select the observation window
and label the run incomplete. Do not infer a hang from missing records.

The report retains a digest, offsets, PCI addresses, event categories and Xid
numbers, not raw kernel messages, process names or hostnames. The tool rejects
oversized input and refuses to overwrite an existing report. A temporal
match does not establish the failing GPU command, which process caused the
fault, or whether the driver or application was responsible. An empty result
does not exclude a fault (journal permissions, retention or missing events).

Next: pair matching device-loss events with the existing application trace
and a synthetic reproducible workload; only then change graphics behavior.

References:
- https://docs.nvidia.com/deploy/xid-errors/latest/working-with-xid-errors.html
- https://www.freedesktop.org/software/systemd/man/latest/journalctl.html

Regression tests: `python -m unittest discover -s tests -p test_gpu_journal.py -v`.
