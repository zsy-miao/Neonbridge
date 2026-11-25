"""Bounded, aggregate-only preparation diagnostics.

The existing triage tool gives detailed emitter reasons, but needs a separate
emitter pass over the archive. These fixed buckets are safe to print during
normal preparation; no shader bytes, digests, raw errors or paths escape.
"""
import re
import subprocess

# Deliberately fixed labels, never interpolated compiler stderr.
REFUSALS = (
    ("wave_divergent", r"wave reduction at 0x[0-9a-f]+ may execute in divergent control flow"),
    ("wave_prologue", r"wave reduction at 0x[0-9a-f]+: unrecognized DPP reduction prologue"),
    ("wave_saveexec", r"wave reduction at 0x[0-9a-f]+: prologue does not start with s_nand_saveexec_b64 sX, 0"),
    ("exec_election", r"unverified EXEC election at 0x[0-9a-f]+: lane is not compared for equality"),
    ("compute_swizzle", r"unsupported compute DS swizzle encoding"),
    ("readlane_uninitialized", r"v_readlane at 0x[0-9a-f]+: v\d+ lane \d+ read before any v_writelane"),
    ("scalar_dynamic_offset", r"s_load at 0x[0-9a-f]+: dynamic soffset unsupported"),
    ("wave_other", r"wave reduction\b"),
    ("exec_other", r"unverified EXEC\b"),
    ("scalar_buffer", r"s_buffer_load\b"),
    ("sampler", r"(?:unsupported|unverified|unmapped)\b[^\n]*\bsampler\b"),
    ("vector_op", r"unsupported vector op\b"),
    ("scalar_op", r"unsupported scalar op\b"),
    ("mesh", r"unverified mesh shader bindings\b"),
    ("mubuf", r"MUBUF\b"),
    # Additional fixed labels for former other_refusal cases; existing
    # primary buckets stay stable for comparisons with earlier receipts.
    ("scalar_code_table_offset", r"s_load at 0x[0-9a-f]+: code-table read with dynamic soffset unsupported"),
    ("mesh_launch", r"(?:unsupported mesh launch or output configuration|unsupported mesh launch register|missing mesh PSV runtime information)"),
    ("mesh_root_layout", r"(?:mesh root signature missing|unrecognized mesh SRV/CBV root-table layout|unverified mesh descriptor|unverified mesh root SRV|mesh root SRV missing)"),
    ("mesh_signature", r"(?:mesh stage requires a full position|unsupported mesh (?:clip-distance|output) signature|mesh primitive attribute exports do not match)"),
    ("mesh_export", r"(?:unsupported (?:packed or masked mesh export|mesh triangle index encoding|mesh primitive attribute export)|missing mesh position export|truncated mesh export sequence|invalid branch in mesh exports)"),
    ("descriptor_mapping", r"(?:unaligned descriptor table offset|descriptor register overflow|descriptor dataflow did not converge|unmapped descriptor|root table slot at .* conflicts)"),
    ("lds_exchange", r"(?:LDS exchange with unverified branch convergence|single-wave LDS exchange inside a loop|guest LDS and exchange exceed the group shared budget|shared exchange requires the native guest LDS allocation)"),
    ("compute_swizzle_other", r"(?:compute swizzle (?:requires complete blocks|following an early exit|inside a loop)|unsupported compute swizzle mode|backward permute requires a verified wave32 compute layout)"),
    ("wave_width", r"(?:conflicting wave widths for ballot and reduction|wave operation requires complete \d+-lane compute waves|resolve Wave32 requires a compatible wave reduction)"),
    ("lane_spill", r"(?:lane op at 0x[0-9a-f]+: non-constant lane operand|is used both as a lane spill area and as a vector|v_readlane at 0x[0-9a-f]+: unexpected destination)"),
    ("heap_atomic", r"atomic through a heap descriptor unsupported"),
    ("packed_root", r"(?:unsupported packed compute root parameter|packed root arguments exceed 64 dwords|root argument offset outside signature|unsupported mutation of a root address|root address or padding used as shader data)"),

)

def refusal_bucket(error):
    if not error.reason_known:
        # A marker written before reasons were kept holds only a key.
        return "cached_reason_unavailable"
    message = str(error)
    # ToolFailure prefixes the emitter stderr. Do not expose it in the report.
    for label, pattern in REFUSALS:
        if re.search(pattern, message, re.IGNORECASE):
            return label
    return "other_refusal"

def failure_bucket(error):
    if isinstance(error, subprocess.TimeoutExpired):
        # TimeoutExpired.cmd may contain private paths; use only fixed labels.
        cmd = error.cmd
        if isinstance(cmd, (tuple, list)) and cmd:
            from pathlib import Path
            tool = Path(cmd[0]).name
            if tool in ("dxc", "dxv", "xshader"):
                return tool + "_timeout"
        return "other_timeout"
    if isinstance(error, OSError):
        return "io_error"
    return "other_failure"

def aggregate(totals, refusal_counts, failure_counts, request_counts):
    """The receipt covers the exact enumerated preparation work, not gameplay."""
    categories = {key: totals[key] for key in ("compiled", "cached", "unsupported", "failed")}
    checked = sum(categories.values())
    if (sum(refusal_counts.values()) != categories["unsupported"]
            or sum(failure_counts.values()) != categories["failed"]
            or sum(request_counts.values()) != checked):
        raise ValueError("incomplete preparation receipt")
    return {
        "schema": 1, "checked": checked, "totals": categories,
        "request_types": dict(sorted(request_counts.items())),
        "refusal_buckets": dict(sorted(refusal_counts.items())),
        "failure_buckets": dict(sorted(failure_counts.items())),
    }
