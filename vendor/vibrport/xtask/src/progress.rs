//! The README's progress tracker: badges and a dashboard of shader
//! translation and game stages, written from `docs/progress/progress.json`
//! between the README's `<!-- progress:start -->` and `<!-- progress:end -->`.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

const START: &str = "<!-- progress:start -->";
const END: &str = "<!-- progress:end -->";

pub fn run(root: &Path) -> Result<()> {
    let data = root.join("docs/progress/progress.json");
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(&data).with_context(|| format!("reading {}", data.display()))?,
    )
    .context("parsing progress.json")?;
    let readme = root.join("README.md");
    let text = std::fs::read_to_string(&readme).context("reading README.md")?;
    let section = render(&json)?;
    let (before, rest) = text
        .split_once(START)
        .context("README.md has no <!-- progress:start --> marker")?;
    let (_, after) = rest
        .split_once(END)
        .context("README.md has no <!-- progress:end --> marker")?;
    std::fs::write(&readme, format!("{before}{START}\n{section}{END}{after}"))
        .context("writing README.md")?;
    println!("README.md progress section updated from {}", data.display());
    Ok(())
}

fn str_of<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key].as_str().with_context(|| format!("progress.json: `{key}` is missing"))
}

/// Percent of `done` in `total`, one decimal, never 100.0 unless all.
fn percent(done: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let p = (done as f64 * 1000.0 / total as f64).floor() / 10.0;
    if done < total { p.min(99.9) } else { 100.0 }
}

fn color(p: f64) -> &'static str {
    match p {
        p if p >= 99.0 => "brightgreen",
        p if p >= 90.0 => "green",
        p if p >= 75.0 => "yellow",
        p if p >= 50.0 => "orange",
        _ => "red",
    }
}

/// A shields.io static badge (label and message escaped for its path).
fn badge(label: &str, message: &str, color: &str) -> String {
    let esc = |s: &str| {
        s.replace('%', "%25")
            .replace('-', "--")
            .replace('_', "__")
            .replace(' ', "%20")
            .replace('/', "%2F")
    };
    format!(
        "![{label}: {message}](https://img.shields.io/badge/{}-{}-{color})",
        esc(label),
        esc(message)
    )
}

/// A 20-cell bar.
fn bar(p: f64) -> String {
    let full = ((p / 5.0).round() as usize).min(20);
    format!("{}{}", "█".repeat(full), "░".repeat(20 - full))
}

fn render(json: &Value) -> Result<String> {
    let shaders = json["shaders"]
        .as_array()
        .context("progress.json: `shaders` is missing")?;
    let mut out = String::new();
    let (mut done_all, mut total_all) = (0, 0);
    let mut rows = String::new();
    let mut badges = Vec::new();
    for s in shaders {
        let stage = str_of(s, "stage")?;
        let done = s["translated"].as_u64().context("progress.json: `translated`")?;
        let total = s["total"].as_u64().context("progress.json: `total`")?;
        done_all += done;
        total_all += total;
        let p = percent(done, total);
        badges.push(badge(&format!("shaders {stage}"), &format!("{p:.1}%"), color(p)));
        rows.push_str(&format!(
            "| Shaders {stage} | `{}` | {p:.1}% | {done} / {total} |\n",
            bar(p)
        ));
    }
    let p_all = percent(done_all, total_all);
    out.push_str(&badge("shaders", &format!("{p_all:.1}%"), color(p_all)));
    out.push('\n');
    for b in badges {
        out.push_str(&b);
        out.push('\n');
    }
    let measured = str_of(json, "measured")?;
    out.push_str(&badge("medido", measured, "blue"));
    out.push_str("\n\n");

    let stages = json["game"].as_array().context("progress.json: `game` is missing")?;
    let reached = stages
        .iter()
        .filter(|g| g["state"].as_str() == Some("works"))
        .count();
    let p_game = percent(reached as u64, stages.len() as u64);
    out.push_str(&format!(
        "| Parte | Progresso | % | Contagem |\n|---|---|---|---|\n\
         | **Todos os shaders** | `{}` | **{p_all:.1}%** | {done_all} / {total_all} |\n{rows}\
         | **Etapas do jogo funcionando** | `{}` | **{p_game:.1}%** | {reached} / {} |\n\n",
        bar(p_all),
        bar(p_game),
        stages.len()
    ));

    out.push_str("| Etapa do jogo | Estado | Medição |\n|---|---|---|\n");
    for g in stages {
        let icon = match str_of(g, "state")? {
            "works" => "✅",
            "partial" => "⚠️",
            _ => "❌",
        };
        out.push_str(&format!(
            "| {} | {icon} | {} |\n",
            str_of(g, "stage")?,
            str_of(g, "note")?
        ));
    }
    out.push_str(&format!(
        "\n*Medido em {measured} em {} (`{}`). As contagens de shaders são os shaders de console \
         do dump do Xbox Series 1.0.2.1 que são traduzidos para este host (`victory translate`); \
         as etapas do jogo são o que uma execução alcançou. Regenere com `cargo xtask progress` \
         depois de editar `docs/progress/progress.json`.*\n",
        str_of(json, "gpu")?,
        str_of(json, "commit")?
    ));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percents_round_down_and_badges_escape() {
        assert_eq!(percent(8703, 8713), 99.8);
        assert_eq!(percent(5, 5), 100.0);
        assert_eq!(percent(9999, 10000), 99.9);
        assert_eq!(bar(50.0).chars().filter(|&c| c == '█').count(), 10);
        assert_eq!(
            badge("vertex/pixel shaders", "97.7%", "green"),
            "![vertex/pixel shaders: 97.7%](https://img.shields.io/badge/vertex%2Fpixel%20shaders-97.7%25-green)"
        );
    }

    #[test]
    fn the_section_lists_every_stage() {
        let json: Value = serde_json::from_str(
            r#"{"measured":"2026-11-24","gpu":"GPU","commit":"abc",
                "shaders":[{"stage":"Mesh","translated":1,"total":2}],
                "game":[{"stage":"Boot","state":"works","note":"30 fps"},
                        {"stage":"World","state":"broken","note":"hangs"}]}"#,
        )
        .unwrap();
        let s = render(&json).unwrap();
        assert!(s.contains("| Shaders Mesh |"));
        assert!(s.contains("| Boot | ✅ | 30 fps |"));
        assert!(s.contains("| World | ❌ | hangs |"));
        assert!(s.contains("1 / 2"));
    }
}
