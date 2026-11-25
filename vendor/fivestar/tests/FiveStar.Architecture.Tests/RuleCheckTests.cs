// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Architecture.Tests.Checking;
using Xunit;

namespace FiveStar.Architecture.Tests;

/// <summary>The checker itself, on tiny hand-made sources: each rule finds what it must and leaves alone what it allows.</summary>
public sealed class RuleCheckTests
{
    [Fact]
    public void A_header_may_follow_a_shebang_and_a_shader_without_one_fails()
    {
        var header = string.Concat(CodePolicy.HeaderLines.Select(line => $"# {line}\n"));
        var script = new SourceFile("tools/run.py", $"#!/usr/bin/env python3\n{header}print()\n");
        var shader = new SourceFile("src/FiveStar.Shaders/Blit.hlsl", "float4 main() : SV_Target { return 0; }\n");

        var violations = Run("FileHeader", script, shader);

        Assert.Equal(["src/FiveStar.Shaders/Blit.hlsl:1"], Places(violations));
    }

    [Fact]
    public void A_block_or_exit_after_other_statements_needs_a_blank_line_above_it_or_its_comment()
    {
        var source = Code("""
            static class C
            {
                static void M(int x)
                {
                    x++;
                    if (x > 0)
                    {
                        return;
                    }

                    // the comment belongs to the exit
                    return;
                }
            }
            """);

        var violations = Run("BlockSpacing", source);

        Assert.Equal(["src/FiveStar.Formats/C.cs:6"], Places(violations));
    }

    [Fact]
    public void Seven_statements_in_a_row_break_the_grouping_and_six_do_not()
    {
        var seven = Code(Method(string.Concat(Enumerable.Repeat("x++;\n", 7))), "Seven.cs");
        var six = Code(Method(string.Concat(Enumerable.Repeat("x++;\n", 6))), "Six.cs");

        var violations = Run("StatementGrouping", seven, six);

        Assert.Equal(["src/FiveStar.Formats/Seven.cs:8"], Places(violations));
    }

    [Fact]
    public void A_conditional_expression_is_allowed_only_as_one_simple_assignment()
    {
        var source = Code("""
            static class C
            {
                static int M(int x)
                {
                    var a = x > 0 ? 1 : 2;
                    a = x > 0 ? a : 2;
                    return x > 0 ? a : (x < 0 ? 1 : 2);
                }
            }
            """);

        var violations = Run("ConditionalExpression", source);

        Assert.Equal(["src/FiveStar.Formats/C.cs:7", "src/FiveStar.Formats/C.cs:7"], Places(violations));
    }

    [Fact]
    public void A_property_pattern_fails_and_type_and_null_patterns_pass()
    {
        var source = Code("""
            static class C
            {
                static bool M(object o) =>
                    o is string text
                    && o is not null
                    && o is { } value;
            }
            """);

        var violations = Run("PropertyPattern", source);

        Assert.Equal(["src/FiveStar.Formats/C.cs:6"], Places(violations));
    }

    [Fact]
    public void A_literal_needs_a_name_outside_tests()
    {
        const string Text = """
            [System.Runtime.CompilerServices.InlineArray(4)]
            struct C
            {
                const int Size = 16;
                static readonly int Half = Size / 2;
                int _first;
                static int M() => 3 * Half + 0 - 1;
            }
            """;

        var violations = Run("MagicNumber", Code(Text), Code(Text, project: "FiveStar.Formats.Tests"));

        Assert.Equal(["src/FiveStar.Formats/C.cs:7"], Places(violations));
    }

    [Fact]
    public void The_namespace_follows_the_folder_below_the_project()
    {
        var wrong = Code("namespace FiveStar.Formats;\n\nstatic class Reader\n{\n}\n", "Archives/Reader.cs");
        var right = Code("namespace FiveStar.Formats.Archives;\n\nstatic class Entry\n{\n}\n", "Archives/Entry.cs");

        var violations = Run("NamespaceFollowsFolder", wrong, right);

        Assert.Equal(["src/FiveStar.Formats/Archives/Reader.cs:1"], Places(violations));
    }

    [Fact]
    public void Classes_are_closed_and_partial_only_for_a_generator()
    {
        var source = Code("""
            using System.Text.RegularExpressions;

            class Open
            {
            }

            static partial class Patterns
            {
                [GeneratedRegex("a+")]
                private static partial Regex Letters();
            }

            sealed partial class Plain
            {
            }
            """);

        var violations = Run("TypeModifiers", source);

        Assert.Equal(["src/FiveStar.Formats/C.cs:13", "src/FiveStar.Formats/C.cs:3"], Places(violations));
    }

    [Fact]
    public void A_constructor_that_subscribes_fails_and_arithmetic_passes()
    {
        var source = Code("""
            sealed class C
            {
                private int _count;

                public C(Source source)
                {
                    _count += 2;
                    source.Changed += OnChanged;
                }

                private void OnChanged()
                {
                }
            }
            """);

        var violations = Run("ConstructorSubscription", source);

        Assert.Equal(["src/FiveStar.Formats/C.cs:8"], Places(violations));
    }

    [Fact]
    public void A_hot_path_method_allocates_nothing_and_a_struct_is_not_an_allocation()
    {
        var source = Code("""
            struct Point
            {
                public Point(int x)
                {
                }
            }

            static class C
            {
                [HotPath]
                static int Step(int[] values, object gate)
                {
                    var point = new Point(1);
                    Span<int> scratch = stackalloc int[4];
                    var copy = new int[4];
                    var name = "step " + values.Length;
                    var count = values.Where(value => value > 0).Count();
                    lock (gate)
                    {
                        return count + new List<int>().Count;
                    }
                }

                static int Cold(int[] values) => values.Select(value => value * 2).Sum();
            }
            """);

        string[] expected =
        [
            "src/FiveStar.Formats/C.cs:15",
            "src/FiveStar.Formats/C.cs:16",
            "src/FiveStar.Formats/C.cs:17",
            "src/FiveStar.Formats/C.cs:17",
            "src/FiveStar.Formats/C.cs:17",
            "src/FiveStar.Formats/C.cs:18",
            "src/FiveStar.Formats/C.cs:20",
        ];

        var violations = Run("HotPathAllocation", source);

        Assert.Equal(expected, Places(violations));
    }

    [Fact]
    public void Only_the_policy_projects_read_the_wall_clock()
    {
        const string Text = "static class C\n{\n    static System.DateTime M() => System.DateTime.UtcNow;\n}\n";

        var violations = Run("AmbientState", Code(Text), Code(Text, project: "FiveStar.Cli"));

        Assert.Equal(["src/FiveStar.Formats/C.cs:3"], Places(violations));
    }

    [Fact]
    public void A_project_references_only_what_its_layer_allows()
    {
        var formats = Project("FiveStar.Formats", "FiveStar.Shaders");
        var cli = Project("FiveStar.Cli", "FiveStar.Formats", "FiveStar.Shaders");
        var tests = Project("FiveStar.Formats.Tests", "FiveStar.Formats");

        var violations = Run("ProjectLayers", formats, cli, tests);

        Assert.Equal(["src/FiveStar.Formats/FiveStar.Formats.csproj:3"], Places(violations));
    }

    [Fact]
    public void A_forbidden_word_fails_in_a_path_and_in_a_line_in_any_case()
    {
        var word = CodePolicy.ForbiddenWords[0];
        var named = new SourceFile($"docs/{word}.md", "# Notes\n");
        var mentioned = new SourceFile("docs/notes.md", $"# Notes\n\nWritten by {word.ToUpperInvariant()}.\n");

        var violations = Run("ForbiddenWords", named, mentioned);

        Assert.Equal([$"docs/{word}.md:1", "docs/notes.md:3"], Places(violations));
    }

    private static IReadOnlyList<Violation> Run(string rule, params SourceFile[] files)
    {
        var withProjects = files.Concat(Projects(files)).ToArray();

        return RuleCatalog.Find(rule).Check(new CodeBase(withProjects));
    }

    /// <summary>A project file for every <c>src/</c> project folder the sources sit in, unless the test passes one.</summary>
    private static IEnumerable<SourceFile> Projects(IReadOnlyList<SourceFile> files)
    {
        var given = files.Where(file => file.Extension == ".csproj").Select(file => file.Name).ToHashSet();

        return files
            .Where(file => file.Path.StartsWith("src/", StringComparison.Ordinal) && file.Path.Count(c => c == '/') >= 2)
            .Select(file => file.Path.Split('/')[1])
            .Distinct()
            .Where(name => !given.Contains(name))
            .Select(name => Project(name));
    }

    private static SourceFile Project(string name, params string[] references)
    {
        var items = string.Concat(references.Select(reference => $"    <ProjectReference Include=\"..\\{reference}\\{reference}.csproj\" />\n"));

        return new SourceFile($"src/{name}/{name}.csproj", $"<Project Sdk=\"Microsoft.NET.Sdk\">\n  <ItemGroup>\n{items}  </ItemGroup>\n</Project>\n");
    }

    private static SourceFile Code(string text, string path = "C.cs", string project = "FiveStar.Formats") =>
        new($"src/{project}/{path}", text);

    private static string Method(string statements) =>
        $"static class C {{ static void M(int x) {{\n{statements}}} }}\n";

    private static string[] Places(IReadOnlyList<Violation> violations) =>
        violations.Select(violation => $"{violation.Path}:{violation.Line}").Order(StringComparer.Ordinal).ToArray();
}
