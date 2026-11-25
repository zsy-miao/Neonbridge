"""Reject unresolved xbox.* intrinsic calls before a PSO submission.

The gate reads the read-only inventory from `probe_roots.xbox_intrinsic_inventory()` and keeps
the three groups apart on purpose: a symbol a module only declares, or one that only metadata
mentions, is not a call and does not block submission. What blocks is a call the module still
executes. A refused module never reaches the submission callable.

A lowering counts only when it is visible in the transformed module: the call is gone from the
instructions, so the inventory no longer reports it. An adapter status table is advisory and
never clears a call the module still emits.
"""

import argparse
import json
from pathlib import Path
import sys

from probe_roots import XBOX_PREFIX, xbox_intrinsic_inventory

LOWERED = "LOWERED"
UNRESOLVED = "UNRESOLVED"
ALLOW = "ALLOW"
REJECT = "REJECT"
NOT_CHECKED = "NOT_CHECKED"


def default_status_of(name):
    """No builtin is lowered in this repository, so every called name stays unresolved."""
    return UNRESOLVED, "no-verified-lowering-in-this-repository"


def status_table(lowered=()):
    """Build an advisory status table: it reports what an adapter claims, nothing more.

    A lowering counts only when it is visible in the transformed module, as a call that is
    gone. What an adapter says about a call it still emits cannot clear that call, so this
    table never reaches the verdict; it only annotates the report.
    """
    lowered = frozenset(lowered)

    def status_of(name):
        if name in lowered:
            return LOWERED, "reported-by-adapter"
        return default_status_of(name)

    return status_of


def reaches_xbox(inventory):
    """True when the module reaches an xbox.* symbol, by calling it or by holding it."""
    return bool(inventory["calls"] or inventory.get("references"))


def decide(inventory, status_of=None):
    """Refuse a module while it still calls or references xbox.*, whatever any adapter reports.

    A call blocks, and so does every other form that keeps the symbol reachable: an alias, a
    stored pointer, a bitcast, a global initializer or a call operand. Each blocking entry names
    its form, so a refusal says how the module still reaches the builtin. The one exception is
    `debug-only`: a symbol carried solely by a debug intrinsic's metadata operands is not
    executable use, so it is reported under `debug_only` and does not block. `status_of` is
    advisory: it fills `advisory` on each entry so a report can show what an adapter claims, and
    the verdict ignores it.
    """
    status_of = status_of or default_status_of
    calls = inventory["calls"]
    references = inventory.get("references") or {}
    blocking_calls, blocking_references, debug_only = [], [], []
    for symbol in sorted(set(calls) | set(references)):
        builtin = symbol[len(XBOX_PREFIX):] if symbol.startswith(XBOX_PREFIX) else symbol
        advisory, unblocks = status_of(builtin)
        entry = {
            "name": symbol,
            "builtin": builtin,
            "status": UNRESOLVED,
            "unblocks": unblocks,
            "advisory": advisory,
        }
        if symbol in calls:
            entry["form"] = "call"
            entry["sites"] = calls[symbol]
            blocking_calls.append(entry)
            continue
        executable = [kind for kind in references[symbol]["kinds"] if kind != "debug-only"]
        if not executable:
            debug_only.append(symbol)
            continue
        entry["form"] = ", ".join(executable)
        entry["sites"] = references[symbol]["sites"]
        blocking_references.append(entry)
    blocking = blocking_calls + blocking_references
    verdict = REJECT if blocking else ALLOW
    names = [entry["name"] for entry in blocking]
    if verdict == REJECT:
        parts = []
        if blocking_calls:
            listed = ", ".join(f"{entry['name']} ({entry['form']})" for entry in blocking_calls)
            parts.append(f"{len(blocking_calls)} executable xbox.* call(s): {listed}")
        if blocking_references:
            listed = ", ".join(f"{entry['name']} ({entry['form']})" for entry in blocking_references)
            parts.append(f"{len(blocking_references)} executable xbox.* reference(s): {listed}")
        message = "REJECT: " + "; ".join(parts)
        claimed = [entry["builtin"] for entry in blocking if entry["advisory"] == LOWERED]
        if claimed:
            message += (f" (advisory reports LOWERED for {', '.join(claimed)};"
                        " the module still reaches them)")
    else:
        message = (
            f"ALLOW: no executable xbox.* calls or references "
            f"(declarations={len(inventory['declarations'])}, metadata_only={len(inventory['metadata_only'])})"
        )
        if debug_only:
            message = message[:-1] + f"; debug-only use of {', '.join(debug_only)})"
    return {
        "verdict": verdict,
        "reason": "executable-xbox-references" if blocking else "no-executable-xbox-references",
        "message": message,
        "calls": calls,
        "declarations": inventory["declarations"],
        "metadata_only": inventory["metadata_only"],
        "references": references,
        "debug_only": debug_only,
        "blocking": blocking,
        "blocking_calls": blocking_calls,
        "blocking_references": blocking_references,
        "reported_names": names,
        "checked": True,
    }


def not_checked(reason):
    """Record that no module was examined, so a caller cannot read it as a cleared module.

    A submission path that has no disassembly of the final module reports this instead of
    gating nothing: the verdict stays distinct from ALLOW and `checked` is False, so a
    caller cannot treat it as cleared.
    """
    return {
        "verdict": NOT_CHECKED,
        "reason": reason,
        "message": f"NOT_CHECKED: the final module was not disassembled ({reason})",
        "checked": False,
        "calls": {},
        "declarations": [],
        "metadata_only": [],
        "blocking": [],
        "reported_names": [],
    }


def check_text(text, status_of=None):
    """Gate a module given as disassembly text."""
    return decide(xbox_intrinsic_inventory(text), status_of)


def check_container(probe, blob, program, status_of=None, optional_modes=()):
    """Gate a DXBC container by disassembling its DXIL part through the validator probe."""
    from probe_roots import validate_shader

    checks = validate_shader(probe, blob, program, optional_modes=optional_modes)
    decision = decide(checks["ir"]["xbox_intrinsics"], status_of)
    decision["validation"] = checks
    return decision


def gate_submission(decision, module_text, submit):
    """Submit only for an ALLOW verdict; REJECT and NOT_CHECKED reach `submit` zero times."""
    decision["submitted"] = decision["verdict"] == ALLOW
    if decision["submitted"]:
        submit(module_text)
    return decision


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command")
    text = sub.add_parser("check-text", help="Gate a module given as disassembly text")
    text.add_argument("path", type=Path, help="Disassembly text file ('-' for standard input)")
    shader = sub.add_parser("check", help="Gate a DXBC container through a validator probe")
    shader.add_argument("input", type=Path, help="DXBC container")
    shader.add_argument("--probe", type=Path, help="root_probe.exe path")
    args = parser.parse_args()

    if args.command not in ("check-text", "check"):
        parser.print_help(sys.stderr)
        return 2

    try:
        if args.command == "check-text":
            body = (sys.stdin.read() if str(args.path) == "-"
                    else args.path.read_text(encoding="utf-8", errors="replace"))
            decision = check_text(body)
        else:
            if not args.probe:
                parser.error("check requires --probe")
            from resource_inventory import parts

            raw = args.input.read_bytes()
            sections = parts(raw)
            if b"DXIL" not in sections:
                raise ValueError("DXBC container has no DXIL part")
            decision = check_container(args.probe.resolve(strict=True), raw, sections[b"DXIL"])
    except (ValueError, OSError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    print(decision["message"])
    print(json.dumps(decision, indent=2))
    return 0 if decision["verdict"] == ALLOW else 1


if __name__ == "__main__":
    sys.exit(main())
