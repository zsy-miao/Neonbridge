// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Dxil.Hlsl;

/// <summary>
/// The C preprocessor subset translated programs use: <c>#include</c> of the runtime headers, object-like
/// <c>#define</c>/<c>#undef</c>, and <c>#if</c>/<c>#ifdef</c>/<c>#ifndef</c>/<c>#elif</c>/<c>#else</c>/<c>#endif</c>.
/// <c>#pragma</c> and <c>#line</c> are ignored; a function-like macro is refused.
/// </summary>
public sealed class Preprocessor
{
    private const int MaxIncludeDepth = 16;
    private const int ParenthesizedName = 2;

    // Room for the runtime headers' tokens, so the output list does not grow through every size.
    private const int IncludedTokens = 1 << 16;

    private readonly Func<string, string?> _includes;
    private readonly Dictionary<string, List<Token>> _macros = new(StringComparer.Ordinal);
    private readonly List<Token> _output = [];
    private readonly Stack<Condition> _conditions = new();

    private Preprocessor(Func<string, string?> includes)
    {
        _includes = includes;
    }

    /// <summary>The tokens of <paramref name="source"/> after preprocessing; <paramref name="includes"/> finds an included file's text.</summary>
    public static List<Token> Run(string source, Func<string, string?> includes)
    {
        var preprocessor = new Preprocessor(includes);
        var tokens = Lexer.Tokenize(source);
        preprocessor._output.Capacity = tokens.Count + IncludedTokens;
        preprocessor.Process(tokens, 0);

        return preprocessor._output;
    }

    private bool Active => _conditions.Count == 0 || _conditions.Peek().Active;

    private void Process(List<Token> tokens, int depth)
    {
        var index = 0;

        while (index < tokens.Count)
        {
            var token = tokens[index];

            if (token.StartsLine && token.Is("#"))
            {
                var end = index + 1;

                while (end < tokens.Count && tokens[end].Line == token.Line)
                {
                    end++;
                }

                Directive(tokens.GetRange(index + 1, end - index - 1), token.Line, depth);
                index = end;

                continue;
            }

            if (Active)
            {
                Expand(token, _output, null);
            }

            index++;
        }
    }

    /// <summary>Appends a token, or the expansion of the macro it names (a macro is not expanded inside itself).</summary>
    private void Expand(Token token, List<Token> output, HashSet<string>? expanding)
    {
        if (token.Kind != TokenKind.Identifier || !_macros.TryGetValue(token.Text, out var body) || (expanding is not null && expanding.Contains(token.Text)))
        {
            output.Add(token);

            return;
        }

        expanding ??= new HashSet<string>(StringComparer.Ordinal);
        expanding.Add(token.Text);

        foreach (var part in body)
        {
            Expand(part with { Line = token.Line, StartsLine = false }, output, expanding);
        }

        expanding.Remove(token.Text);
    }

    private void Directive(List<Token> line, int number, int depth)
    {
        if (line.Count == 0)
        {
            return;
        }

        var name = line[0].Text;
        var rest = line.GetRange(1, line.Count - 1);

        switch (name)
        {
            case "if":
                Open(Active && Evaluate(rest, number) != 0);
                break;
            case "ifdef":
                Open(Active && _macros.ContainsKey(rest[0].Text));
                break;
            case "ifndef":
                Open(Active && !_macros.ContainsKey(rest[0].Text));
                break;
            case "elif":
                Else(rest, number);
                break;
            case "else":
                Else(null, number);
                break;
            case "endif":
                _conditions.Pop();
                break;
            default:
                ActiveDirective(name, rest, number, depth);
                break;
        }
    }

    private void ActiveDirective(string name, List<Token> rest, int number, int depth)
    {
        if (!Active)
        {
            return;
        }

        switch (name)
        {
            case "define":
                Define(rest, number);
                break;
            case "undef":
                _macros.Remove(rest[0].Text);
                break;
            case "include":
                Include(rest, number, depth);
                break;
            case "pragma":
            case "line":
                break;
            default:
                throw new DxilBackendException($"line {number}: #{name} is not supported");
        }
    }

    private void Open(bool active)
    {
        _conditions.Push(new Condition(active, active, Active));
    }

    /// <summary>#elif (with a condition) or #else (without).</summary>
    private void Else(List<Token>? condition, int number)
    {
        if (_conditions.Count == 0)
        {
            throw new DxilBackendException($"line {number}: #else without #if");
        }

        var current = _conditions.Pop();
        var take = current.Outer && !current.Taken && (condition is null || EvaluateOuter(condition, number, current.Outer));
        _conditions.Push(new Condition(take, current.Taken || take, current.Outer));
    }

    private bool EvaluateOuter(List<Token> condition, int number, bool outer)
    {
        if (!outer)
        {
            return false;
        }

        return Evaluate(condition, number) != 0;
    }

    private void Define(List<Token> rest, int number)
    {
        var name = rest[0];

        if (rest.Count > 1 && rest[1].Is("(") && rest[1].Offset == name.Offset + name.Text.Length)
        {
            throw new DxilBackendException($"line {number}: function-like macro {name.Text} is not supported");
        }

        _macros[name.Text] = rest.GetRange(1, rest.Count - 1);
    }

    private void Include(List<Token> rest, int number, int depth)
    {
        if (rest.Count == 0 || rest[0].Kind != TokenKind.Text)
        {
            throw new DxilBackendException($"line {number}: #include without a quoted name");
        }

        var text = _includes(rest[0].Text) ?? throw new DxilBackendException($"line {number}: include {rest[0].Text} not found");

        if (depth >= MaxIncludeDepth)
        {
            throw new DxilBackendException($"line {number}: includes nested too deeply");
        }

        Process(Lexer.Tokenize(text), depth + 1);
    }

    /// <summary>An #if expression: <c>defined</c> is resolved, macros are expanded and unknown names are 0.</summary>
    private long Evaluate(List<Token> condition, int number)
    {
        var expanded = new List<Token>();

        for (var index = 0; index < condition.Count; index++)
        {
            if (condition[index].Is("defined"))
            {
                var parenthesized = index + 1 < condition.Count && condition[index + 1].Is("(");
                var nameIndex = parenthesized ? index + ParenthesizedName : index + 1;
                ulong defined = _macros.ContainsKey(condition[nameIndex].Text) ? 1UL : 0UL;
                expanded.Add(new Token(TokenKind.Integer, "defined", number, false) { Integer = defined });
                index = parenthesized ? nameIndex + 1 : nameIndex;

                continue;
            }

            Expand(condition[index], expanded, null);
        }

        return new ConditionEvaluator(expanded, number).Evaluate();
    }

    private readonly record struct Condition(bool Active, bool Taken, bool Outer);
}
