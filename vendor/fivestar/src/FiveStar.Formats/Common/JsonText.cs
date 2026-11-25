// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;

namespace FiveStar.Formats.Common;

/// <summary>
/// Writes a value tree as indented JSON. Trees hold <see langword="null"/>, bool, integers, double, string,
/// <see cref="IReadOnlyList{T}"/> of object and <see cref="OrderedMap{TKey, TValue}"/> of string to object.
/// Doubles use <see cref="DecimalText.Shortest"/>; non-finite doubles are written as NaN / Infinity, non-ASCII
/// characters as \u escapes.
/// </summary>
public static class JsonText
{
    private const char FirstPrintable = ' ';
    private const char LastAscii = '~';

    public static string Write(object? value, int indent)
    {
        var text = new StringBuilder();
        WriteValue(text, value, indent, 0);

        return text.ToString();
    }

    private static void WriteValue(StringBuilder text, object? value, int indent, int level)
    {
        switch (value)
        {
            case null:
                text.Append("null");
                break;
            case bool flag:
                WriteBool(text, flag);
                break;
            case double number:
                WriteDouble(text, number);
                break;
            case string str:
                WriteString(text, str);
                break;
            case OrderedMap<string, object?> map:
                WriteMap(text, map, indent, level);
                break;
            case IReadOnlyList<object?> list:
                WriteList(text, list, indent, level);
                break;
            case IFormattable integer:
                text.Append(integer.ToString(null, CultureInfo.InvariantCulture));
                break;
            default:
                throw new ArgumentException("unsupported JSON value " + value.GetType().Name, nameof(value));
        }
    }

    private static void WriteBool(StringBuilder text, bool flag)
    {
        var word = flag ? "true" : "false";
        text.Append(word);
    }

    private static void WriteDouble(StringBuilder text, double number)
    {
        if (double.IsNaN(number))
        {
            text.Append("NaN");

            return;
        }

        if (double.IsInfinity(number))
        {
            var infinity = number > 0 ? "Infinity" : "-Infinity";
            text.Append(infinity);

            return;
        }

        text.Append(DecimalText.Shortest(number));
    }

    private static void WriteList(StringBuilder text, IReadOnlyList<object?> list, int indent, int level)
    {
        if (list.Count == 0)
        {
            text.Append("[]");

            return;
        }

        text.Append('[');

        for (var index = 0; index < list.Count; index++)
        {
            Separate(text, index, indent, level + 1);
            WriteValue(text, list[index], indent, level + 1);
        }

        NewLine(text, indent, level);
        text.Append(']');
    }

    private static void WriteMap(StringBuilder text, OrderedMap<string, object?> map, int indent, int level)
    {
        if (map.Count == 0)
        {
            text.Append("{}");

            return;
        }

        text.Append('{');

        for (var index = 0; index < map.Count; index++)
        {
            Separate(text, index, indent, level + 1);
            WriteString(text, map.Keys[index]);
            text.Append(": ");
            WriteValue(text, map.Values[index], indent, level + 1);
        }

        NewLine(text, indent, level);
        text.Append('}');
    }

    private static void Separate(StringBuilder text, int index, int indent, int level)
    {
        if (index > 0)
        {
            text.Append(',');
        }

        NewLine(text, indent, level);
    }

    private static void NewLine(StringBuilder text, int indent, int level)
    {
        text.Append('\n').Append(' ', indent * level);
    }

    private static void WriteString(StringBuilder text, string value)
    {
        text.Append('"');

        foreach (var character in value)
        {
            WriteCharacter(text, character);
        }

        text.Append('"');
    }

    private static void WriteCharacter(StringBuilder text, char character)
    {
        switch (character)
        {
            case '"':
                text.Append("\\\"");
                break;
            case '\\':
                text.Append("\\\\");
                break;
            case '\n':
                text.Append("\\n");
                break;
            case '\r':
                text.Append("\\r");
                break;
            case '\t':
                text.Append("\\t");
                break;
            case '\b':
                text.Append("\\b");
                break;
            case '\f':
                text.Append("\\f");
                break;
            default:
                WritePlainCharacter(text, character);
                break;
        }
    }

    private static void WritePlainCharacter(StringBuilder text, char character)
    {
        if (character < FirstPrintable || character > LastAscii)
        {
            text.Append("\\u").Append(((int)character).ToString("x4", CultureInfo.InvariantCulture));

            return;
        }

        text.Append(character);
    }
}
