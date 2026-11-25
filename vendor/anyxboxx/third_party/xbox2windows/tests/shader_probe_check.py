"""Exercise both desktop validator modes, text analysis, and the native root reader using owned shaders."""

import json
from pathlib import Path
import subprocess
import sys

import pefile

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from probe_roots import packing_evidence, parts, validate_shader

SKIP = 77
E_INVALIDARG = "0x80070057"
VALID = {"call_hr": "0x00000000", "status": "0x00000000"}


def file_version(path):
    with pefile.PE(str(path), fast_load=True) as pe:
        pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_RESOURCE"]])
        info = pe.VS_FIXEDFILEINFO[0]
    return info.FileVersionMS >> 16, info.FileVersionMS & 0xFFFF, info.FileVersionLS >> 16, info.FileVersionLS & 0xFFFF


def module_outcome(records, version):
    """Classify the module-only records of both control shaders.

    DXC 1.8.2407 and older ship a dxil.dll numbered 101.x that returns E_INVALIDARG for
    DxcValidatorFlags_ModuleOnly. Starting with 1.8.2502 the same call validates the module.
    Only a 101.x validator may refuse, and it must refuse both shaders.
    """
    if len(records) == 2 and all(r.get("status") == "0x00000000" for r in records):
        return "validated"
    refused = len(records) == 2 and all("status" not in r and r["call_hr"] == E_INVALIDARG for r in records)
    if not refused or version[0] != 101:
        raise AssertionError(f"Unexpected module-only validation result: {records} from validator {version}")
    return "refused"


def check_controls():
    refused, legacy, newer = {"call_hr": E_INVALIDARG}, (101, 8, 2407, 12), (1, 9, 2609, 5)
    accepted = {
        "validated_by_newer_validator": ([VALID, VALID], newer, "validated"),
        "refused_by_101_validator": ([refused, refused], legacy, "refused"),
    }
    rejected = {
        "newer_validator_refusal": ([refused, refused], newer),
        "first_accepting_release_refusal": ([refused, refused], (1, 8, 2502, 8)),
        "refused_then_validated": ([refused, VALID], legacy),
        "validated_then_refused": ([VALID, refused], legacy),
        "other_call_failure": ([{"call_hr": "0x80004005"}] * 2, legacy),
        "failed_module_status": ([{"call_hr": "0x00000000", "status": "0x80aa0009"}, VALID], newer),
        "single_record": ([refused], legacy),
    }
    for name, (records, version, expected) in accepted.items():
        assert module_outcome(records, version) == expected, name
        print(f"control {name}: {expected}")
    for name, (records, version) in rejected.items():
        try:
            module_outcome(records, version)
        except AssertionError:
            print(f"control {name}: rejected")
        else:
            raise AssertionError(f"control {name} was accepted")


def main():
    check_controls()
    if len(sys.argv) == 1:
        return 0
    positive = "{ i32, i32, i32 } { i32 2139095039, i32 2139095039, i32 2139095039 }"
    negative = positive.replace("2139095039", "-8388609")
    store = "call void @xbox.global_store_dwordx3_with_cache(i32 362, i64 "
    fixture = f"""
define void @first() {{
    %sel = and i32 %index, 3
    %v = call i32 @xbox.V_ALIGNBYTE_B32(i32 332, i32 %a, i32 %b, i32 %sel)
    {store}%base, {positive}, i32 undef, i32 undef)
    %end = add i64 %base, 12
    {store}%end, {negative}, i32 undef, i32 undef)
    {store}%other, {positive}, i32 undef, i32 undef)
    {store}%end, {negative}, i32 undef, i32 undef)
}}
define void @second() {{
    %v = call i32 @xbox.V_ALIGNBYTE_B32(i32 332, i32 %a, i32 %b, i32 %sel)
}}
"""
    evidence = packing_evidence(fixture)
    assert evidence["alignbyte_selectors"] == {"and_3": 1, "other_or_unknown": 1}
    assert evidence["adjacent_extrema_store_pairs"] == 1
    assert evidence["store_addresses"] == {"dwordx3:other_or_unknown": 2, "dwordx3:add_12": 2}
    probe, root, tables, without_root, validator = map(Path, sys.argv[1:])
    modules = []
    for path, params, samplers in ((root, 3, 0), (tables, 6, 1)):
        data = path.read_bytes()
        sections = parts(data)
        result = json.loads(subprocess.check_output([str(probe)], input=data))
        assert result["legacy_hr"] == result["versioned_hr"] == result["original_serialize_hr"] == "0x00000000"
        assert (result["parameters"], result["samplers"], result["flags"]) == (params, samplers, 0)
        checks = validate_shader(probe, data, sections[b"DXIL"], optional_modes=("module",))
        assert checks["container"]["status"] == "0x00000000"
        modules.append({key: checks["module"][key] for key in ("call_hr", "status") if key in checks["module"]})
        assert checks["ir"]["target_triple"] == "dxil-ms-dx" and not checks["ir"]["xbox_call_sites"]
        assert checks["ir"]["dx_op_call_sites"]
    result = json.loads(subprocess.check_output([str(probe)], input=without_root.read_bytes()))
    assert result["legacy_hr"] != "0x00000000" and result["versioned_hr"] != "0x00000000"
    assert "original_serialize_hr" not in result
    version = file_version(validator)
    outcome = module_outcome(modules, version)
    print(f"validator={validator.name} file_version={'.'.join(map(str, version))} module_only={outcome}")
    if outcome == "refused":
        print(f"Module-only validation skipped: the validator returned {E_INVALIDARG}. "
              "Configure with DXC 1.8.2502 or newer to run it.")
        return SKIP
    return 0


if __name__ == "__main__":
    sys.exit(main())
