// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Buffers.Binary;

namespace FiveStar.Formats.Platform;

/// <summary>
/// Finds the GDK thunks compiled into a module: a <c>call</c> preceded by <c>lea rcx, [clsid]</c> and
/// <c>lea rdx, [iid]</c> (or <c>mov rdx, rcx</c>), followed by a vtable call. The call goes to QueryApiImpl, either
/// through an import or through a function pointer the module resolves at run time.
/// </summary>
public static class GdkThunkScanner
{
    private const string CodeSectionPrefix = ".text";
    private const string ConstantSection = ".rdata";
    private const byte CallRel32 = 0xE8;
    private const int CallLength = 5;
    private const int CallDisplacement = 1;
    private const int GuidSize = 16;

    // The GUID loads sit within a few instructions of the query call; the vtable call follows within ~40 instructions.
    private const int LookBack = 40;
    private const int LookAhead = 192;

    // A module that uses the GDK has many thunks; a target reached by fewer thunk-shaped calls is not QueryApiImpl.
    private const int MinimumQuerySites = 2;

    /// <summary>The distinct (clsid, iid, slot) thunks of a module whose class is in <paramref name="knownClasses"/>, unnamed.</summary>
    public static IReadOnlyList<GdkThunk> Scan(PeImage image, IReadOnlySet<Guid> knownClasses)
    {
        return [.. Candidates(image).Where(candidate => knownClasses.Contains(candidate.Thunk.Clsid)).Select(candidate => candidate.Thunk).Distinct()];
    }

    /// <summary>
    /// Every thunk's call to QueryApiImpl, with no library needed: QueryApiImpl is the one target most thunk-shaped
    /// calls with constant GUIDs reach. Calls through a vtable that only look like a thunk go elsewhere and drop out.
    /// </summary>
    public static IReadOnlyList<GdkThunkSite> ScanSites(PeImage image)
    {
        var candidates = Candidates(image).Where(candidate => candidate.ConstantGuids).ToList();
        var query = QueryTarget(candidates);

        if (query is null)
        {
            return [];
        }

        return [.. candidates.Where(candidate => candidate.Target == query.Value).Select(candidate => new GdkThunkSite(candidate.Thunk, candidate.Rva))];
    }

    /// <summary>The RVA of the module's QueryApiImpl (or of the function that forwards to it), or null when none.</summary>
    public static uint? QueryFunction(PeImage image)
    {
        return QueryTarget(Candidates(image).Where(candidate => candidate.ConstantGuids).ToList());
    }

    private static uint? QueryTarget(IReadOnlyList<GdkThunkCandidate> candidates)
    {
        var busiest = candidates.GroupBy(candidate => candidate.Target).MaxBy(group => group.Count());

        if (busiest is null || busiest.Count() < MinimumQuerySites)
        {
            return null;
        }

        return busiest.Key;
    }

    private static List<GdkThunkCandidate> Candidates(PeImage image)
    {
        var candidates = new List<GdkThunkCandidate>();

        foreach (var section in image.Sections.Where(section => section.Name.StartsWith(CodeSectionPrefix, StringComparison.Ordinal)))
        {
            var start = (int)image.RawStart(section);
            var length = (int)Math.Min(section.RawSize, section.VirtualSize);
            var code = image.Data.AsSpan(start, Math.Min(length, image.Data.Length - start));

            for (var at = 0; at + CallLength <= code.Length; at++)
            {
                if (code[at] != CallRel32)
                {
                    continue;
                }

                var candidate = CandidateAt(image, section, code, at);

                if (candidate is not null)
                {
                    candidates.Add(candidate);
                }
            }
        }

        return candidates;
    }

    private static GdkThunkCandidate? CandidateAt(PeImage image, PeSection section, ReadOnlySpan<byte> code, int call)
    {
        uint? clsid = null;
        uint? iid = null;
        var first = call;

        // Nearest lea per register, walking back from the call.
        for (var lea = call - GdkThunkCode.LeaLength; lea >= Math.Max(0, call - LookBack); lea--)
        {
            var target = GdkThunkCode.LeaTarget(code, lea);

            if (target == GdkRegister.Rcx && clsid is null)
            {
                clsid = LeaRva(section, code, lea);
                first = lea;
            }
            else if (target == GdkRegister.Rdx && iid is null)
            {
                iid = LeaRva(section, code, lea);
            }
        }

        if (clsid is null)
        {
            return null;
        }

        if (iid is null && GdkThunkCode.CopiesRcxToRdx(code, first, call))
        {
            iid = clsid;
        }

        var slot = GdkThunkCode.FirstApiSlot(code, call + CallLength, call + CallLength + LookAhead);
        var clsidGuid = GuidAt(image, clsid.Value);
        Guid? iidGuid = null;

        if (iid is not null)
        {
            iidGuid = GuidAt(image, iid.Value);
        }

        if (clsidGuid is null || iidGuid is null || slot is null)
        {
            return null;
        }

        var thunk = new GdkThunk(clsidGuid.Value, iidGuid.Value, slot.Value, null);
        var rva = (uint)(section.VirtualAddress + call);
        var callTarget = (uint)(rva + CallLength + BinaryPrimitives.ReadInt32LittleEndian(code[(call + CallDisplacement)..]));
        var constant = InConstants(image, clsid.Value) && InConstants(image, iid!.Value);

        return new GdkThunkCandidate(thunk, rva, callTarget, constant);
    }

    private static uint LeaRva(PeSection section, ReadOnlySpan<byte> code, int lea)
    {
        var displacement = BinaryPrimitives.ReadInt32LittleEndian(code[(lea + GdkThunkCode.LeaDisplacement)..]);

        return (uint)(section.VirtualAddress + lea + GdkThunkCode.LeaLength + displacement);
    }

    private static Guid? GuidAt(PeImage image, uint rva)
    {
        var offset = image.Offset(rva);

        if (offset is null || offset.Value + GuidSize > image.Data.Length)
        {
            return null;
        }

        return new Guid(image.Data.AsSpan((int)offset.Value, GuidSize));
    }

    private static bool InConstants(PeImage image, uint rva)
    {
        foreach (var section in image.Sections)
        {
            if (rva >= section.VirtualAddress && rva + GuidSize <= section.VirtualAddress + section.VirtualSize)
            {
                return string.Equals(section.Name, ConstantSection, StringComparison.Ordinal);
            }
        }

        return false;
    }
}
