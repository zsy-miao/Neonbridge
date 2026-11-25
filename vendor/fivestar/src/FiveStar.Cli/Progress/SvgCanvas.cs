// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Security;
using System.Text;

namespace FiveStar.Cli.Progress;

/// <summary>Collects the SVG elements of the report; colours come from CSS classes so both themes share the markup.</summary>
internal sealed class SvgCanvas
{
    private readonly StringBuilder _body = new();

    public string Body => _body.ToString();

    public static string Escape(string text)
    {
        return SecurityElement.Escape(text);
    }

    public void Rect(string cssClass, double x, double y, double width, double height, double radius)
    {
        _body.Append("<rect class=\"").Append(cssClass).Append('"');

        Attribute("x", x);
        Attribute("y", y);
        Attribute("width", width);
        Attribute("height", height);
        Attribute("rx", radius);
        _body.Append("/>\n");
    }

    /// <summary>A transparent rectangle over a group that darkens on hover and carries its exact numbers.</summary>
    public void HitArea(double x, double y, double width, double height, string title)
    {
        _body.Append("<rect class=\"hit\"");
        Attribute("x", x);
        Attribute("y", y);
        Attribute("width", width);
        Attribute("height", height);
        _body.Append("><title>").Append(Escape(title)).Append("</title></rect>\n");
    }

    public void Text(string cssClass, double x, double y, double size, string text)
    {
        TextElement(cssClass, x, y, size, string.Empty, text);
    }

    public void BoldText(string cssClass, double x, double y, double size, string text)
    {
        TextElement(cssClass, x, y, size, " font-weight=\"600\"", text);
    }

    private void TextElement(string cssClass, double x, double y, double size, string extra, string text)
    {
        _body.Append("<text class=\"").Append(cssClass).Append('"');
        Attribute("x", x);
        Attribute("y", y);
        Attribute("font-size", size);
        _body.Append(extra).Append('>').Append(Escape(text)).Append("</text>\n");
    }

    private void Attribute(string name, double value)
    {
        _body.Append(' ').Append(name).Append("=\"").Append(ProgressText.Coordinate(value)).Append('"');
    }
}
