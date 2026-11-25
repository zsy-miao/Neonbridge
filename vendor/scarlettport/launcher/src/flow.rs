//! Guided Play flow: stage state machine and progress-line parsing for the runtime's output.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    GameNotSet,
    ShadersNotBuilt,
    BuildingShaders,
    Ready,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum World {
    /// Shaders are not ready yet.
    Waiting,
    /// No journal: play once to record world pipelines.
    NeedPlay,
    /// Journal present and not built yet (or it grew since).
    Available,
    Building,
    Done,
}

pub struct Inputs<'a> {
    pub game_complete: bool,
    pub precompiling: bool,
    pub built_hash: &'a str,
    /// None while the runtime is unpacking: staleness is then unknown and not reported.
    pub runtime_hash: Option<&'a str>,
    /// The engine builds its shaders while you play: nothing to build up front.
    pub inline_shaders: bool,
}

pub fn shaders_current(i: &Inputs) -> bool {
    i.inline_shaders || (!i.built_hash.is_empty() && i.runtime_hash.is_none_or(|h| h == i.built_hash))
}

pub fn stage(i: &Inputs) -> Stage {
    if i.precompiling {
        Stage::BuildingShaders
    } else if !i.game_complete {
        Stage::GameNotSet
    } else if shaders_current(i) {
        Stage::Ready
    } else {
        Stage::ShadersNotBuilt
    }
}

pub fn world(stage: Stage, building: bool, journal_len: Option<u64>, built_len: u64) -> World {
    if building {
        World::Building
    } else if stage != Stage::Ready {
        World::Waiting
    } else {
        match journal_len {
            None => World::NeedPlay,
            Some(l) if l == built_len => World::Done,
            Some(_) => World::Available,
        }
    }
}

/// `  6800/26503 (42 s)` -> (6800, 26503, 42)
pub fn parse_shader(line: &str) -> Option<(u64, u64, u64)> {
    let t = line.trim();
    let (a, rest) = t.split_once('/')?;
    if a.is_empty() || !a.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let total: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let inner = rest[total.len()..].trim_start().strip_prefix('(')?;
    let secs: String = inner.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !inner[secs.len()..].trim_start().starts_with('s') {
        return None;
    }
    Some((a.parse().ok()?, total.parse().ok()?, secs.parse().ok()?))
}

/// `  120 done, 880 left (3.5/s, ~4 min to go)` -> (120, 880)
pub fn parse_pipelines_progress(line: &str) -> Option<(u64, u64)> {
    let t = line.trim();
    let (done, rest) = t.split_once(" done, ")?;
    let (left, _) = rest.split_once(" left")?;
    Some((done.trim().parse().ok()?, left.trim().parse().ok()?))
}

/// Rate from the first sample of this run, so a resumed build's earlier work does not skew it.
#[derive(Default)]
pub struct Eta {
    first: Option<(u64, u64)>,
}

impl Eta {
    pub fn reset(&mut self) {
        self.first = None;
    }

    pub fn remaining_secs(&mut self, done: u64, total: u64, secs: u64) -> Option<u64> {
        let (d0, s0) = *self.first.get_or_insert((done, secs));
        let left = total.saturating_sub(done);
        if done > d0 && secs > s0 {
            Some(left * (secs - s0) / (done - d0))
        } else {
            None
        }
    }
}

pub fn duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {} min", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{} min", secs.div_ceil(60))
    } else {
        format!("{secs} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp<'a>(game: bool, pre: bool, built: &'a str, rt: Option<&'a str>) -> Inputs<'a> {
        Inputs { game_complete: game, precompiling: pre, built_hash: built, runtime_hash: rt, inline_shaders: false }
    }

    #[test]
    fn parses_shader_lines() {
        assert_eq!(parse_shader("  6800/26503 (42 s)"), Some((6800, 26503, 42)));
        assert_eq!(parse_shader("2600/26503 (718 s)"), Some((2600, 26503, 718)));
        assert_eq!(parse_shader("a/b (1 s)"), None);
        assert_eq!(parse_shader("12/34 (5 x)"), None);
        assert_eq!(parse_shader("hello"), None);
        assert_eq!(parse_pipelines_progress("  120 done, 880 left (3.5/s, ~4 min to go)"), Some((120, 880)));
        assert_eq!(parse_pipelines_progress("nothing"), None);
    }

    #[test]
    fn eta() {
        let mut e = Eta::default();
        assert_eq!(e.remaining_secs(6000, 10000, 100), None); // resumed: no rate yet
        assert_eq!(e.remaining_secs(6100, 10000, 110), Some(3900 * 10 / 100));
        let mut f = Eta::default();
        assert_eq!(f.remaining_secs(0, 1000, 0), None);
        assert_eq!(f.remaining_secs(100, 1000, 10), Some(90));
        assert_eq!(duration(7500), "2 h 5 min");
        assert_eq!(duration(90), "2 min");
    }

    #[test]
    fn stages() {
        assert_eq!(stage(&inp(false, false, "", None)), Stage::GameNotSet);
        assert_eq!(stage(&inp(true, false, "", Some("h"))), Stage::ShadersNotBuilt);
        assert_eq!(stage(&inp(true, true, "", Some("h"))), Stage::BuildingShaders);
        assert_eq!(stage(&inp(true, false, "h", Some("h"))), Stage::Ready);
        // a new runtime marks shaders stale
        assert_eq!(stage(&inp(true, false, "old", Some("new"))), Stage::ShadersNotBuilt);
        // runtime still unpacking: trust the stored hash
        assert_eq!(stage(&inp(true, false, "old", None)), Stage::Ready);
        assert_eq!(stage(&inp(false, false, "h", Some("h"))), Stage::GameNotSet);
    }

    #[test]
    fn inline_shaders_are_ready() {
        let i = Inputs { game_complete: true, precompiling: false, built_hash: "", runtime_hash: Some("h"), inline_shaders: true };
        assert_eq!(stage(&i), Stage::Ready);
        let i = Inputs { game_complete: false, ..i };
        assert_eq!(stage(&i), Stage::GameNotSet);
    }

    #[test]
    fn world_steps() {
        assert_eq!(world(Stage::ShadersNotBuilt, false, Some(99), 0), World::Waiting);
        assert_eq!(world(Stage::Ready, false, None, 0), World::NeedPlay);
        assert_eq!(world(Stage::Ready, false, Some(99), 0), World::Available);
        assert_eq!(world(Stage::Ready, false, Some(99), 99), World::Done);
        assert_eq!(world(Stage::Ready, false, Some(120), 99), World::Available);
        assert_eq!(world(Stage::Ready, true, Some(99), 0), World::Building);
    }
}
