// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation;

/// <summary>
/// A translated mesh program, in any of its forms (two-phase, staged or superstep), with probe code (rdna_prelude.hlsli,
/// FIVESTAR_MESH_PROBE): every load site (a <c>BufLoad</c>, <c>BufLoadRW</c>, <c>FmtLoad</c> or <c>PtrLoad</c> call, with
/// the range check before it: <c>buf_fits</c>, or the compare of an s_buffer_load) calls the probe form with its site
/// number, which marks the site in a per-thread mask when its range check fails, it reads through an unmapped page of
/// the address map, or through a record naming no descriptor. The register a site's line loads is noted the first time
/// on thread 0, m0 at GS_ALLOC_REQ (from which the vertex and primitive counts come), vertex 0's position and the culled
/// primitives as they are written, and at the end of the group thread 0 appends a <see cref="MeshProbeRecord"/> to a ring
/// in the GPU-memory window. The same line in both phases of a program gets the same site numbers. Site 31 also stands
/// for every site past it. A pixel or compute program gets the same site probes (FIVESTAR_STAGE_PROBE): each invocation
/// keeps its own notes, and at its end the first lane of each wave appends a record with the wave's site masks, its own
/// values and window addresses, its pixel (x, y) or group, and the colour a pixel program returns.
/// </summary>
public static partial class MeshProbe
{
    /// <summary>The define of probed programs; its value is <see cref="Id"/> of the program.</summary>
    public const string Define = "FIVESTAR_MESH_PROBE";

    /// <summary>The define of probed pixel and compute programs (per-invocation notes, a record per wave).</summary>
    public const string StageDefine = "FIVESTAR_STAGE_PROBE";

    /// <summary>The ring: a count, then records from <see cref="RecordsOffset"/> (rdna_prelude PROBE_RECORDS).</summary>
    public const uint Records = 4096;
    public const uint RecordsOffset = 16;
    public const uint RecordBytes = MeshProbeRecord.Dwords * sizeof(uint);

    /// <summary>Window bytes the ring takes.</summary>
    public const uint RingBytes = RecordsOffset + (Records * RecordBytes);

    private const string MeshEntry = "out vertices Out verts[";
    private const string PixelEntry = "\nOut main(";
    private const string ComputeEntry = "void main(uint3 in_gid : SV_GroupID, uint3 in_gtid : SV_GroupThreadID";
    private const string PixelReturn = "  return o;";
    private const string GroupStart = "g_ms_nv = 0; g_ms_np = 0; g_ms_alloc = 0; }";
    private const string Allocation = "g_ms_alloc = 1; }";
    private const string BufferCheck = "buf_fits";
    private const string ScalarCheck = "sbuf_fits";
    private const string ProbePrefix = "probe_";
    private const int MaxSiteText = 160;
    private const uint FnvOffset = 2166136261;
    private const uint FnvPrime = 16777619;

    /// <summary>A stable id of a program name (32-bit FNV-1a), the same in every boot.</summary>
    public static uint Id(string name)
    {
        uint hash = FnvOffset;

        foreach (char c in name)
        {
            hash = (hash ^ c) * FnvPrime;
        }

        return hash;
    }

    /// <summary>
    /// The program with probe code writing to the ring at byte <paramref name="ringOffset"/> of the GPU-memory window,
    /// and the text of each load site by number. Throws <see cref="UnsupportedProgramException"/> for a program that is
    /// not a mesh, pixel or compute program.
    /// </summary>
    public static (string Hlsl, IReadOnlyList<string> Sites) Instrument(string hlsl, uint id, uint ringOffset)
    {
        if (!hlsl.Contains(MeshEntry, StringComparison.Ordinal))
        {
            return InstrumentStage(hlsl, id, ringOffset);
        }

        int body = BodyStart(hlsl);
        var sites = new List<string>();
        var text = hlsl[..body] + NumberSites(hlsl[body..], sites);
        int start = text.IndexOf(GroupStart, StringComparison.Ordinal);
        int end = text.TrimEnd().LastIndexOf('}');

        if (start < 0 || end < 0)
        {
            throw new UnsupportedProgramException("no mesh group start or end to probe");
        }

        text = text[..end] + "  probe_end(in_gid, in_gi, g_ms_nv, g_ms_np);\n" + text[end..];
        text = text.Insert(start + GroupStart.Length, " probe_begin(in_gi);");
        text = text.Replace(Allocation, "g_ms_alloc = 1; probe_alloc(s_m0); }", StringComparison.Ordinal);
        text = NotePosition(text);
        text = CullPattern().Replace(text, "$0 probe_cull(in_gi, ($1 >> 31) != 0, g_ms_np);");
        var defines = string.Create(CultureInfo.InvariantCulture, $"#define {Define} {id}u\n#define {Define}_BASE {ringOffset}u\n#define {Define}_SITES {sites.Count}u\n");

        return (defines + text, sites);
    }

    /// <summary>A pixel or compute program with the site probes and a record per wave at the end of each invocation.</summary>
    private static (string Hlsl, IReadOnlyList<string> Sites) InstrumentStage(string hlsl, uint id, uint ringOffset)
    {
        int body = BodyStart(hlsl);
        var sites = new List<string>();
        var text = hlsl[..body] + NumberSites(hlsl[body..], sites);

        if (text.Contains(ComputeEntry, StringComparison.Ordinal))
        {
            int end = text.TrimEnd().LastIndexOf('}');
            text = text[..end] + "  probe_end_stage(in_gid, float4(0.0f, 0.0f, 0.0f, 0.0f), false);\n" + text[end..];
        }
        else if (text.Contains(PixelEntry, StringComparison.Ordinal) && text.Contains(PixelReturn, StringComparison.Ordinal))
        {
            text = text.Replace(PixelReturn, $"  probe_end_stage({PixelAt(text)}, {PixelColour(text)});\n{PixelReturn}", StringComparison.Ordinal);
        }
        else
        {
            throw new UnsupportedProgramException("the probe is for mesh, pixel and compute programs");
        }

        var defines = string.Create(CultureInfo.InvariantCulture, $"#define {StageDefine} 1\n#define {Define} {id}u\n#define {Define}_BASE {ringOffset}u\n#define {Define}_SITES {sites.Count}u\n");

        return (defines + text, sites);
    }

    /// <summary>The pixel a pixel program's invocation shades, from its SV_Position input; (0, 0) without one.</summary>
    private static string PixelAt(string text)
    {
        var position = PixelPositionPattern().Match(text);

        if (!position.Success)
        {
            return "uint3(0u, 0u, 0u)";
        }

        return $"uint3(uint2({position.Groups["name"].Value}.xy), 0u)";
    }

    /// <summary>The colour a pixel program returns in its first float4 target, and whether there is one.</summary>
    private static string PixelColour(string text)
    {
        var target = ColourTargetPattern().Match(text);

        if (!target.Success)
        {
            return "float4(0.0f, 0.0f, 0.0f, 0.0f), false";
        }

        return $"o.{target.Groups["name"].Value}, true";
    }

    /// <summary>Where the program's own code starts: past its last #include line.</summary>
    private static int BodyStart(string hlsl)
    {
        var include = IncludePattern().Matches(hlsl);

        if (include.Count == 0)
        {
            return 0;
        }

        var last = include[^1];

        return last.Index + last.Length;
    }

    /// <summary>Vertex 0's position where it is written: whole (staged) or as its member (direct export).</summary>
    private static string NotePosition(string text)
    {
        var position = PositionPattern().Match(text);

        if (!position.Success)
        {
            return text;
        }

        var member = position.Groups["name"].Value;
        text = text.Replace($"verts[in_gi].{member} = _e;", $"verts[in_gi].{member} = _e; if (in_gi == 0u) probe_position(_e);", StringComparison.Ordinal);

        return text.Replace("verts[in_gi] = o;", $"verts[in_gi] = o; if (in_gi == 0u) probe_position(o.{member});", StringComparison.Ordinal);
    }

    /// <summary>
    /// Each load call as its probe form with a site number, an s_buffer_load's compare as a check; equal lines get equal
    /// numbers. A line that loads a register notes its value under the line's first site.
    /// </summary>
    private static string NumberSites(string body, List<string> sites)
    {
        var known = new Dictionary<string, List<int>>(StringComparer.Ordinal);
        var lines = body.Split('\n');

        for (int i = 0; i < lines.Length; i++)
        {
            var line = ScalarCheckPattern().Replace(lines[i], $"select({ScalarCheck}(${{cond}}), ${{load}}(");
            var key = line.Trim();

            if (!LoadPattern().IsMatch(key))
            {
                continue;
            }

            if (!known.TryGetValue(key, out var numbers))
            {
                numbers = LineSites(line, sites);
                known[key] = numbers;
            }

            lines[i] = Number(line, numbers);
            var register = RegisterPattern().Match(line);

            if (register.Success && numbers.Count > 0)
            {
                lines[i] += string.Create(CultureInfo.InvariantCulture, $" probe_value({numbers[0]}u, {register.Groups["reg"].Value});");
            }
        }

        return string.Join('\n', lines);
    }

    private static string Number(string line, List<int> numbers)
    {
        int next = 0;
        string? previous = null;

        return LoadPattern().Replace(line, call =>
        {
            var name = call.Groups["name"].Value;
            int site;

            // A load's check and load are one site: the load takes the number of the check before it.
            if (IsCheck(previous) && !IsCheck(name))
            {
                site = numbers[next - 1];
            }
            else
            {
                site = numbers[next++];
            }

            previous = name;

            return string.Create(CultureInfo.InvariantCulture, $"{ProbePrefix}{name}({site}u, ");
        });
    }

    /// <summary>New site numbers for the sites of one line, with their text.</summary>
    private static List<int> LineSites(string line, List<string> sites)
    {
        var numbers = new List<int>();
        string? previous = null;

        foreach (Match call in LoadPattern().Matches(line))
        {
            var name = call.Groups["name"].Value;

            if (IsCheck(previous) && !IsCheck(name))
            {
                previous = name;

                continue;
            }

            numbers.Add(sites.Count);
            sites.Add(CallText(line, call.Index));
            previous = name;
        }

        return numbers;
    }

    private static bool IsCheck(string? name)
    {
        return name is BufferCheck or ScalarCheck;
    }

    /// <summary>A call's text up to its closing parenthesis (shortened).</summary>
    private static string CallText(string line, int start)
    {
        int depth = 0;
        var text = new StringBuilder();

        for (int i = start; i < line.Length && text.Length < MaxSiteText; i++)
        {
            text.Append(line[i]);
            depth += line[i] switch
            {
                '(' => 1,
                ')' => -1,
                _ => 0,
            };

            if (depth == 0 && line[i] == ')')
            {
                break;
            }
        }

        return text.ToString();
    }

    [GeneratedRegex(@"\b(?<name>sbuf_fits|buf_fits|BufLoadRW|BufLoad|FmtLoad|PtrLoad)\(")]
    private static partial Regex LoadPattern();

    /// <summary>An s_buffer_load dword: <c>select(offset + k &lt; limit, BufLoad(...), 0u)</c>.</summary>
    [GeneratedRegex(@"select\((?<cond>[^,()]+ < [^,()]+), (?<load>BufLoadRW|BufLoad)\(")]
    private static partial Regex ScalarCheckPattern();

    /// <summary>A line that sets a register: <c>s4 = ...;</c> or <c>if (active) v2 = ...;</c>.</summary>
    [GeneratedRegex(@"^\s*(?:if \(active\) )?(?<reg>[sv]\d+) = [^;]*;\s*$")]
    private static partial Regex RegisterPattern();

    [GeneratedRegex(@"^#include ""[^""]+""[^\n]*\n", RegexOptions.Multiline)]
    private static partial Regex IncludePattern();

    [GeneratedRegex(@"float4 (?<name>\w+) : SV_Position\b", RegexOptions.IgnoreCase)]
    private static partial Regex PositionPattern();

    [GeneratedRegex(@"prims\[in_gi\]\.cull = \((_?p) >> 31\) != 0(?: \|\| any\(_i >= _n\))?;")]
    private static partial Regex CullPattern();

    /// <summary>A pixel program's SV_Position parameter of main.</summary>
    [GeneratedRegex(@"Out main\([^)]*float4 (?<name>\w+) : SV_Position")]
    private static partial Regex PixelPositionPattern();

    /// <summary>The first float4 render target of a pixel program's Out struct.</summary>
    [GeneratedRegex(@"float4 (?<name>\w+) : SV_Target\d")]
    private static partial Regex ColourTargetPattern();
}
