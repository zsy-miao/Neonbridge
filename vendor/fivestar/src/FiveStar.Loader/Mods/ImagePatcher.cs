// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security.Cryptography;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims;

namespace FiveStar.Loader.Mods;

/// <summary>
/// Applies code patches to an image after it is mapped, relocated and bound, while its pages are still writable and
/// before any of its code runs. A patch writes either every expected match or nothing.
/// </summary>
internal sealed unsafe class ImagePatcher
{
    private const char SymbolSeparator = '!';

    private readonly ShimTable _shims;
    private readonly LoaderLog _log;
    private readonly object _gate = new();
    private readonly List<CodePatch> _patches = [];
    private readonly List<PatchOutcome> _outcomes = [];

    public ImagePatcher(ShimTable shims, LoaderLog log)
    {
        _shims = shims;
        _log = log;
    }

    /// <summary>Every outcome so far, in the order the patches were tried.</summary>
    public IReadOnlyList<PatchOutcome> Outcomes
    {
        get
        {
            lock (_gate)
            {
                return [.. _outcomes];
            }
        }
    }

    /// <summary>Adds patches for images mapped from now on.</summary>
    public void Add(IEnumerable<CodePatch> patches)
    {
        lock (_gate)
        {
            _patches.AddRange(patches);
        }
    }

    /// <summary>Applies every patch whose module is <paramref name="image"/>, in the order they were added.</summary>
    public void Apply(LoadedImage image)
    {
        lock (_gate)
        {
            var mine = _patches.Where(p => string.Equals(p.Module, image.Name, StringComparison.OrdinalIgnoreCase)).ToList();
            string? hash = null;

            foreach (var patch in mine)
            {
                if (patch.Sha256 is not null)
                {
                    hash ??= Convert.ToHexString(SHA256.HashData(image.Pe.Data));
                }

                var outcome = ApplyOne(image, patch, hash);
                _outcomes.Add(outcome);
                Log(outcome);
            }
        }
    }

    private PatchOutcome ApplyOne(LoadedImage image, CodePatch patch, string? hash)
    {
        if (!patch.Enabled)
        {
            return Outcome(image, patch, PatchStatus.Skipped, "disabled");
        }

        if (patch.Sha256 is not null && !string.Equals(patch.Sha256, hash, StringComparison.OrdinalIgnoreCase))
        {
            return Outcome(image, patch, PatchStatus.Skipped, $"module SHA-256 is {hash}, the patch wants {patch.Sha256}");
        }

        byte[]? replacement = Replacement(patch);

        if (replacement is null)
        {
            return Outcome(image, patch, PatchStatus.Failed, $"no shim {patch.JumpTarget}");
        }

        var memory = new Span<byte>((void*)image.Base, (int)image.Size);
        var matches = patch.Pattern.FindAll(memory, patch.ExpectedMatches + 1);

        if (matches.Count != patch.ExpectedMatches)
        {
            return Outcome(image, patch, PatchStatus.Failed, MatchCountError(matches.Count, patch.ExpectedMatches));
        }

        foreach (int match in matches)
        {
            long start = (long)match + patch.Offset;

            if (start < 0 || start + replacement.Length > memory.Length)
            {
                return Outcome(image, patch, PatchStatus.Failed, $"offset {patch.Offset} from +0x{match:x} leaves the image");
            }
        }

        foreach (int match in matches)
        {
            replacement.CopyTo(memory[(match + patch.Offset)..]);
        }

        string where = string.Join(", ", matches.Select(m => $"+0x{m + patch.Offset:x}"));

        return Outcome(image, patch, PatchStatus.Applied, $"{replacement.Length} bytes at {where}");
    }

    /// <summary>The bytes to write: the patch's own, or <c>mov rax,shim; jmp rax</c>. Null when the shim is unknown.</summary>
    private byte[]? Replacement(CodePatch patch)
    {
        if (patch.Bytes is not null)
        {
            return patch.Bytes;
        }

        string target = patch.JumpTarget!;
        int separator = target.IndexOf(SymbolSeparator, StringComparison.Ordinal);
        var shim = _shims.TryResolve(target[..separator], target[(separator + 1)..], 0);

        if (shim is null)
        {
            return null;
        }

        var code = new X64Emitter();
        code.MovRax((ulong)shim.Address);
        code.JmpRax();

        return code.ToArray();
    }

    private static string MatchCountError(int found, int expected)
    {
        if (found == 0)
        {
            return "pattern not found";
        }

        if (found > expected)
        {
            return $"pattern matched more than {expected} time(s)";
        }

        return $"pattern matched {found} time(s), expected {expected}";
    }

    private static PatchOutcome Outcome(LoadedImage image, CodePatch patch, PatchStatus status, string detail)
    {
        return new PatchOutcome(patch, image.Name, status, detail);
    }

    private void Log(PatchOutcome outcome)
    {
        var patch = outcome.Patch;
        string message = $"  [patch] {patch.Name} ({patch.Source}) on {outcome.Module}: {outcome.Status.ToString().ToLowerInvariant()}, {outcome.Detail}";

        if (outcome.Status == PatchStatus.Failed)
        {
            _log.Warn(message.TrimStart());

            return;
        }

        _log.Info(message);
    }
}
