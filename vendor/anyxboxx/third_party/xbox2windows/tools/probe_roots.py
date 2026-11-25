"""Measure desktop deserialization of the containers indexed by call_evidence.py."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
from tempfile import NamedTemporaryFile

import pefile

from call_evidence import NAMES
from resource_inventory import parts

PROJECT = Path(__file__).resolve().parents[1]

XBOX_PREFIX = "xbox."
CALL_SITE = re.compile(
    r'^\s*(?:%(?:"[^"]+"|[\w.$-]+)\s*=\s*)?(?:(?:musttail|tail|notail)\s+)?'
    r'call\b[^@\n]*@(?:"([^"]+)"|([\w.$-]+))\s*\('
)
DECLARATION = re.compile(r'^declare\b[^@\n]*@(?:"([^"]+)"|([\w.$-]+))\s*\(')
# A call instruction, with the optional assigned name and the tail/musttail/notail modifier.
# No leading block label is matched: `dxc -dumpbin` prints a basic block as a `; <label>:N`
# comment line of its own and never prefixes an instruction with one, so a label cannot appear in
# front of a call in this input (0 such lines in the disassembly of the source-built fixtures).
CALL_INSTRUCTION = re.compile(
    r'^\s*(?:%(?:"[^"]+"|[\w.$-]+)\s*=\s*)?(?:(?:musttail|tail|notail)\s+)?call\b')
# Every @ symbol on a line: the callee of a direct or bitcast-spelled call is the first one that
# appears before the operand list, and any other xbox.* symbol is a reference to the same effect.
CALLEE = re.compile(r'@(?:"([^"]+)"|([\w.$-]+))')
ALIAS_DEFINITION = re.compile(r'^@(?:"[^"]+"|[\w.$-]+)\s*=\s*alias\b')
STORE_INSTRUCTION = re.compile(r'^(?:%[^\s=]+\s*=\s*)?store\b')
BITCAST_USE = re.compile(r'\bbitcast\b')
GLOBAL_INITIALIZER = re.compile(
    r'^@(?:"[^"]+"|[\w.$-]+)\s*=\s*(?:global|constant|externally_initialized)\b')
# Debug intrinsics take metadata operands that the runtime never reads, so an xbox.* symbol that
# only appears inside one of these calls is not an executable use. Only the callee decides: a line
# that merely mentions a debug intrinsic somewhere else is still an executable call.
DEBUG_INTRINSIC = re.compile(r'llvm\.dbg\.(?:value|declare|assign|label)\b')
XBOX_SYMBOL = re.compile(r"xbox\.[\w.$-]+")
METADATA_STRING = re.compile(r'!"([^"]*)"')


def strip_comment(line):
    """Drop a trailing ';' comment, keeping ';' inside a quoted string.

    LLVM IR comments start at ';', but a metadata string value or a quoted symbol name may
    contain one, and truncating there loses an intrinsic name.
    """
    quoted = False
    for index, char in enumerate(line):
        if char == '"':
            quoted = not quoted
        elif char == ";" and not quoted:
            return line[:index]
    return line


def reference_kind(line):
    """Name the executable form of a non-call xbox.* reference.

    A call to a debug intrinsic is not executable use and is labelled `debug-only` by
    `classify_line()` instead, so this helper never sees it.
    """
    if ALIAS_DEFINITION.match(line):
        return "alias"
    if STORE_INSTRUCTION.match(line):
        return "store"
    if BITCAST_USE.search(line):
        return "bitcast"
    if GLOBAL_INITIALIZER.match(line):
        return "global-initializer"
    return "operand"


def call_operand_paren(line):
    """Index of the call's operand list, which is the last top-level '('.

    Everything before it is the callee expression, so an xbox.* symbol there is the callee of a
    direct or bitcast-spelled call; a symbol after it is an operand of that call.
    """
    depth, position = 0, -1
    for index, char in enumerate(line):
        if char == "(":
            if depth == 0:
                position = index
            depth += 1
        elif char == ")":
            depth = max(0, depth - 1)
    return position


def classify_line(line, calls, declarations, references):
    """Sort one non-metadata line into a declaration, a call site, or a reference."""
    if line.startswith("declare"):
        declaration = DECLARATION.match(line)
        if declaration:
            symbol = declaration[1] or declaration[2]
            if symbol.startswith(XBOX_PREFIX):
                declarations.add(symbol)
        return
    is_call = CALL_INSTRUCTION.match(line) is not None
    operands_at = call_operand_paren(line) if is_call else -1
    callee_at = None
    if is_call:
        for match in CALLEE.finditer(line):
            if operands_at < 0 or match.start() < operands_at:
                callee_at = match.start()
                callee_name = match.group(1) or match.group(2)
                break
    debug_call = callee_at is not None and DEBUG_INTRINSIC.match(callee_name) is not None
    for match in CALLEE.finditer(line):
        symbol = match.group(1) or match.group(2)
        if not symbol.startswith(XBOX_PREFIX):
            continue
        if match.start() == callee_at:
            calls[symbol] += 1
            continue
        if debug_call:
            kind = "debug-only"
        elif is_call:
            kind = "call-operand"
        else:
            kind = reference_kind(line)
        entry = references.setdefault(symbol, {"sites": 0, "debug_sites": 0, "kinds": set()})
        # sites counts executable uses only; debug-only uses are counted apart.
        if kind == "debug-only":
            entry["debug_sites"] += 1
        else:
            entry["sites"] += 1
        entry["kinds"].add(kind)


def xbox_intrinsic_inventory(text):
    """Classify xbox.* intrinsic names in the disassembler's textual IR.

    A call is an instruction whose callee is the symbol; a declaration is only the function table
    entry; a name carried by a metadata string value is metadata-only. An unused declaration, or a
    name that metadata alone mentions, is therefore never reported as a call. A named-metadata key
    such as !xbox.compiler.options names a node, not an intrinsic, and is never counted.

    `references` covers every other way the module keeps a symbol reachable: an alias, a stored
    pointer, a bitcast, a global initializer or a call operand. Those forms are not calls, but they
    reach the same builtin once the module runs, so they are reported with their kind.
    """
    declarations, calls, metadata_names, references = set(), Counter(), set(), {}
    for raw in text.splitlines():
        line = strip_comment(raw).strip()
        if not line:
            continue
        if not line.startswith("!"):
            classify_line(line, calls, declarations, references)
        for value in METADATA_STRING.findall(line):
            metadata_names.update(XBOX_SYMBOL.findall(value))
    return dict(
        calls=dict(sorted(calls.items())),
        declarations=sorted(declarations),
        metadata_only=sorted(metadata_names - declarations - set(calls) - set(references)),
        references={symbol: {"sites": item["sites"],
                              "debug_sites": item["debug_sites"],
                              "kinds": sorted(item["kinds"])}
                    for symbol, item in sorted(references.items())},
    )


def packing_evidence(text):
    """Summarize immediate SSA definitions in the SDK disassembler's textual IR."""
    selectors = Counter()
    addresses = Counter()
    adjacent_extrema = 0
    definitions = {}
    previous_positive = None
    local = r"%[\w.$-]+"
    for raw in text.splitlines():
        line = raw.partition(";")[0].strip()
        if line.startswith("define ") or line == "}":
            definitions.clear()
            previous_positive = None
        definition = re.match(rf"({local}) = (.+)", line)
        if definition:
            definitions[definition[1]] = definition[2]
        if "call " not in line:
            continue
        if "@xbox.V_ALIGNBYTE_B32(" in line:
            selector = re.search(rf", i32 ({local})\)", line)
            producer = definitions.get(selector[1], "") if selector else ""
            masked = re.fullmatch(rf"and i32 (?:{local}, 3|3, {local})", producer)
            selectors["and_3" if masked else "other_or_unknown"] += 1
        store = re.search(rf"@xbox\.global_store_(dword(?:x[34])?)_with_cache\(i32 \d+, i64 ({local}), (.*)", line)
        if not store:
            continue
        width, address, payload = store.groups()
        producer = definitions.get(address, "")
        addition = re.fullmatch(rf"add i64 ({local}), (-?\d+|{local})", producer)
        if addition:
            kind = "add_variable" if addition[2].startswith("%") else "add_" + addition[2]
        else:
            kind = "extractvalue" if producer.startswith("extractvalue ") else "other_or_unknown"
        addresses[f"{width}:{kind}"] += 1
        # Only count a second triple whose address actually uses the preceding
        # triple's SSA address. Equal offsets on separate bases are insufficient.
        literal = re.match(r"\{ i32, i32, i32 \} \{ i32 (-?\d+), i32 \1, i32 \1 \}", payload)
        extremum = int(literal[1]) if width == "dwordx3" and literal else None
        if extremum == -8388609 and addition and addition[1] == previous_positive and addition[2] == "12":
            adjacent_extrema += 1
        previous_positive = address if extremum == 2139095039 else None
    return dict(alignbyte_selectors=dict(selectors), store_addresses=dict(addresses),
                adjacent_extrema_store_pairs=adjacent_extrema)


def validate_shader(probe, blob, program, optional_modes=()):
    """Validate the container and its bitcode. A refused optional mode keeps its record without a status."""
    if len(program) < 24:
        raise ValueError("Truncated DXIL program")
    _, words, magic, _, offset, size = struct.unpack_from("<II4sIII", program)
    start = 8 + offset
    if (magic != b"DXIL" or offset < 16 or size < 4 or not start + size <= 4 * words <= len(program)
            or program[start:start + 4] != b"BC\xc0\xde"):
        raise ValueError("DXIL program header does not describe a bitcode module")
    checks = {}
    for mode, data in (("container", blob), ("module", program[start:start + size])):
        result = subprocess.run([str(probe), "--" + mode], input=data, capture_output=True,
                                check=True, timeout=10)
        record = json.loads(result.stdout)
        if "status" not in record and mode not in optional_modes:
            raise ValueError(f"Validator operation unavailable for {mode}: {record}")
        record["diagnostics"] = result.stderr.decode("utf-8", "replace").rstrip("\0\r\n").splitlines()
        checks[mode] = record
    text = subprocess.run([str(probe), "--disassemble"], input=program[start:start + size],
                          capture_output=True, check=True, timeout=10).stdout.decode("utf-8", "replace")
    triple = re.search(r'^target triple\s*=\s*"([^"]+)"', text, re.M)
    if not triple:
        raise ValueError("DXC disassembly did not identify a target triple")
    standard = Counter()
    operand_forms = {}
    for line in text.splitlines():
        instruction = line.partition(";")[0]
        call = CALL_SITE.search(instruction)
        if call:
            symbol = call[1] or call[2]
            if symbol.startswith(XBOX_PREFIX):
                operands = instruction[call.end():].split(")", 1)[0]
                form = re.sub(r"%[\w.]+", "%value", operands)
                operand_forms.setdefault(symbol, Counter())[form] += 1
            elif symbol.startswith("dx.op."):
                standard[symbol] += 1
    intrinsics = xbox_intrinsic_inventory(text)
    checks["ir"] = dict(target_triple=triple[1], xbox_call_sites=intrinsics["calls"],
                        xbox_intrinsics=intrinsics, dx_op_call_sites=dict(sorted(standard.items())),
                        operand_forms=operand_forms, packing=packing_evidence(text))
    return checks


def summarize_operands(modules):
    summary = {}
    for ir in modules:
        for symbol, forms in ir["operand_forms"].items():
            record = summary.setdefault(symbol, {"modules": 0, "sites": 0, "forms": Counter()})
            record["modules"] += 1
            record["sites"] += sum(forms.values())
            record["forms"].update(forms)
    return summary


def measure(build, inventory, probe, fixtures):
    result = {}
    for name in NAMES:
        image = inventory[name]
        if not image["containers"]:
            continue
        data = (build / name).read_bytes()
        sha = hashlib.sha256(data).hexdigest()
        if sha != image["sha256"]:
            raise ValueError(f"Rebuild the call inventory for {name}")
        rows = []
        with pefile.PE(data=data, fast_load=True) as pe:
            for item in image["containers"]:
                blob = pe.get_data(int(item["rva"], 16), item["bytes"])
                sections = parts(blob)
                signature = sections[b"RTS0"]
                native = subprocess.run([str(probe)], input=blob, capture_output=True, check=True, timeout=10)
                row = {"rva": item["rva"], "root_sha256": hashlib.sha256(signature).hexdigest(),
                       "native": json.loads(native.stdout)}
                flags, = struct.unpack_from("<I", signature, 20)
                desktop = signature[:20] + struct.pack("<I", flags & ~0x03000000) + signature[24:]
                row["matching_desktop_fixtures"] = [key for key, payload in fixtures.items() if payload == desktop]
                if b"DXIL" in sections:
                    token, = struct.unpack_from("<I", sections[b"DXIL"])
                    row["program_kind"] = token >> 16
                    row["shader_model"] = f"{(token >> 4) & 15}.{token & 15}"
                    row["validation"] = validate_shader(probe, blob, sections[b"DXIL"])
                rows.append(row)
        keys = ("legacy_hr", "versioned_hr", "original_serialize_hr", "without_observed_bits_hr")
        validation = {mode: dict(Counter(r["validation"][mode]["status"] for r in rows if "validation" in r))
                      for mode in ("container", "module")}
        ir = [r["validation"]["ir"] for r in rows if "validation" in r]
        symbols = Counter(symbol for r in ir for symbol in r["xbox_call_sites"])
        result[name] = dict(sha256=sha, containers=len(rows), unique_root_payloads=len({r["root_sha256"] for r in rows}),
                            dxil_validation=validation,
                            modules_with_xbox_calls=sum(bool(r["xbox_call_sites"]) for r in ir),
                            modules_per_xbox_symbol=dict(symbols.most_common()),
                            static_operands=summarize_operands(ir),
                            fixture_matches=dict(Counter(key for r in rows for key in r["matching_desktop_fixtures"])),
                            results={k: dict(Counter(r["native"].get(k, "not_tested") for r in rows)) for k in keys},
                            entries=rows)
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("build", type=Path)
    p.add_argument("--inventory", type=Path, default=PROJECT / "out/calls/six-apis.json")
    p.add_argument("--probe", type=Path, default=PROJECT / "out/build/dev/root_probe.exe")
    p.add_argument("--controls", type=Path, help="Build directory with compute_root.cso and compute_tables.cso")
    args = p.parse_args()
    dest = PROJECT / "out/calls/desktop-roots.json"
    build = args.build.resolve(strict=True)
    if dest.resolve().is_relative_to(build) or dest.resolve() == args.inventory.resolve():
        p.error("Report destination overlaps inputs")
    fixtures = {}
    control_checks = {}
    probe = args.probe.resolve(strict=True)
    if args.controls:
        for name in ("compute_root", "compute_tables"):
            blob = (args.controls / (name + ".cso")).read_bytes()
            sections = parts(blob)
            fixtures[name] = sections[b"RTS0"]
            checks = validate_shader(probe, blob, sections[b"DXIL"])
            if any(checks[k]["status"] != "0x00000000" for k in ("container", "module")):
                raise ValueError(f"Desktop positive control failed for {name}")
            control_checks[name] = checks
    inventory_data = args.inventory.read_bytes()
    result = measure(build, json.loads(inventory_data), probe, fixtures)
    document = dict(schema=1, windows_build=sys.getwindowsversion().build,
                    probe_sha256=hashlib.sha256(probe.read_bytes()).hexdigest(),
                    inventory_sha256=hashlib.sha256(inventory_data).hexdigest(),
                    control_root_sha256={key: hashlib.sha256(b).hexdigest() for key, b in fixtures.items()},
                    control_validation=control_checks, images=result)
    dest.parent.mkdir(parents=True, exist_ok=True)
    with NamedTemporaryFile(mode="w", dir=dest.parent, delete=False, encoding="utf-8") as f:
        json.dump(document, f, indent=2)
        f.write("\n")
    Path(f.name).replace(dest)
    for name, report in result.items():
        print(name, report["containers"], report["results"])


if __name__ == "__main__":
    main()
