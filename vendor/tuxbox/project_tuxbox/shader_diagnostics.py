"""Shared shader refusal grouping for preparation and offline triage."""
import re

MASKS = (
    # "[buffer_load_dwordx3 @ 0x1a0]" names one variant of the same gap.
    (re.compile(r"\s*\[[a-z0-9_]+ @ 0x[0-9a-fA-F]+\]"), ""),
    (re.compile(r"0x[0-9a-fA-F]+"), "0x_"),
    (re.compile(r"\b([svt])\[?\d+(?::\d+)?\]?"), r"\1N"),
    (re.compile(r"\b\d+\b"), "N"),
)


def refusal_line(message):
    """The emitter's reason: the last non-empty line, without the tool prefix."""
    message = re.sub(r"^xshader(?:\.exe)? failed \(65\):\s*", "", message)
    return next((part.strip() for part in reversed(message.splitlines()) if part.strip()), "")


def normalize_refusal(message):
    """One line per refusal, with program counters, registers and counts masked."""
    line = refusal_line(message)
    for pattern, replacement in MASKS:
        line = pattern.sub(replacement, line)
    return line or "(no message)"

# Retain the instruction named by the emitter before normalization removes
# location-specific suffixes. Unknown messages remain grouped by reason.
OPCODE = re.compile(r"\[([a-z][a-z0-9_]+) @ 0x[0-9a-fA-F]+\]")
LEADING_OPCODE = re.compile(r"^((?:s|v|buffer|image|ds|flat|global|scratch|tbuffer)_[a-z0-9_]+)\b")
SAMPLER = re.compile(
    r"not a sampler descriptor .* words \["
    r"(0x[0-9a-fA-F]{1,8}), (0x[0-9a-fA-F]{1,8}), (0x[0-9a-fA-F]{1,8}), (0x[0-9a-fA-F]{1,8})\]$"
)


def refusal_detail(message):
    """Preserve useful distinctions without copying native shader code."""
    line = refusal_line(message)
    match = OPCODE.search(line) or LEADING_OPCODE.match(line)
    words = SAMPLER.search(line)
    return {
        "reason": normalize_refusal(line),
        "opcode": match.group(1) if match else None,
        "sampler_words": [f"{int(word, 16):#x}" for word in words.groups()] if words else None,
    }
