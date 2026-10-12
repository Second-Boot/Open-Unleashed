//! The command line, read once at start-up (`main::run`).
//!
//! A small hand parser, not a library: every rule below is the one `run` applied to the raw
//! arguments before this struct existed, kept as it was — a flag's value is the token after the
//! flag's **first** occurrence (whatever that token is), a value that does not parse counts as
//! absent, a flag given without its value is still "present" for the presence tests, and a
//! positional argument is any token that is neither a flag nor the value of a flag that takes one.
//!
//! ```text
//! porsche [track] [car] [sim] [--fe[=layout]] [--layout NAME] [--layout-sweep] [--debug-menu]
//!         [--screenshot out.png] [--autotest SECS [--follow]] [--opponents N] [--laps N]
//!         [--quick-knockout] [--backwards] [--mirror] [--record run.csv] [--replay FILE]
//!         [--career ID] [--mission]
//!         [--net host[:PORT] | --net join:HOST[:PORT] [--net-name NAME]] [--net-browse SECS]
//! ```

/// The flags that take a value: the token after one of them is never a positional argument.
const FLAGS_WITH_VALUE: [&str; 11] = [
    "--screenshot",
    "--autotest",
    "--opponents",
    "--record",
    "--laps",
    "--career",
    "--layout",
    "--replay",
    "--net",
    "--net-name",
    "--net-browse",
];

/// The flags that make a run scripted (no focus taken, see `NO_FOCUS`), by presence alone.
const SCRIPTED_FLAGS: [&str; 4] = ["--autotest", "--screenshot", "--layout-sweep", "--record"];

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Cli {
    /// `[track] [car] [sim]`: the tokens that are neither flags nor flag values
    pub positional: Vec<String>,
    /// `--replay <file>`: play back a recording the port saved
    pub replay: Option<String>,
    /// `--career <id>`: start the next event of tournament `<id>` from the saved career
    pub career: Option<i16>,
    /// `--mission`: start the current Factory Driver mission headlessly
    pub mission: bool,
    /// `--autotest` given (whatever its value)
    pub autotest_flag: bool,
    /// `--autotest <secs>`
    pub autotest: Option<f32>,
    /// `--follow`
    pub follow: bool,
    /// `--opponents <n>`
    pub opponents: Option<u32>,
    /// `--laps <n>`
    pub laps: Option<u32>,
    /// `--quick-knockout`
    pub quick_knockout: bool,
    /// `--record <file.csv>`
    pub record: Option<String>,
    /// `--backwards`
    pub backwards: bool,
    /// `--mirror`
    pub mirror: bool,
    /// `--screenshot <file.png>`
    pub screenshot: Option<String>,
    /// `--layout-sweep`
    pub layout_sweep: bool,
    /// `--fe` / `--fe=<layout>`: the layout named, `QuickRace` when none is (the first token
    /// starting with `--fe`)
    pub fe: Option<String>,
    /// `--debug-menu`
    pub debug_menu: bool,
    /// `--layout` given (whatever its value)
    pub layout_flag: bool,
    /// `--layout <name>`
    pub layout: Option<String>,
    /// `--net` given (whatever its value)
    pub net_flag: bool,
    /// `--net host[:PORT]` / `--net join:HOST[:PORT]`
    pub net: Option<String>,
    /// `--net-name <name>`
    pub net_name: Option<String>,
    /// `--net-browse <secs>`
    pub net_browse: Option<f32>,
    /// one of `--autotest`, `--screenshot`, `--layout-sweep`, `--record` given
    pub scripted: bool,
}

impl Cli {
    /// The process's own arguments, the program name skipped.
    pub(crate) fn from_env() -> Self {
        Self::parse(&std::env::args().skip(1).collect::<Vec<_>>())
    }

    pub(crate) fn parse(args: &[String]) -> Self {
        let has = |flag: &str| args.iter().any(|a| a == flag);
        let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
        let mut positional = Vec::new();
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
                continue;
            }
            if a.starts_with("--") {
                skip = FLAGS_WITH_VALUE.contains(&a.as_str());
                continue;
            }
            positional.push(a.clone());
        }
        Self {
            positional,
            replay: value("--replay").cloned(),
            career: value("--career").and_then(|v| v.parse().ok()),
            mission: has("--mission"),
            autotest_flag: has("--autotest"),
            autotest: value("--autotest").and_then(|v| v.parse().ok()),
            follow: has("--follow"),
            opponents: value("--opponents").and_then(|v| v.parse().ok()),
            laps: value("--laps").and_then(|v| v.parse().ok()),
            quick_knockout: has("--quick-knockout"),
            record: value("--record").cloned(),
            backwards: has("--backwards"),
            mirror: has("--mirror"),
            screenshot: value("--screenshot").cloned(),
            layout_sweep: has("--layout-sweep"),
            fe: args.iter().find(|a| a.starts_with("--fe")).map(|a| match a.split_once('=') {
                Some((_, name)) if !name.is_empty() => name.to_string(),
                _ => "QuickRace".to_string(),
            }),
            debug_menu: has("--debug-menu"),
            layout_flag: has("--layout"),
            layout: value("--layout").cloned(),
            net_flag: has("--net"),
            net: value("--net").cloned(),
            net_name: value("--net-name").cloned(),
            net_browse: value("--net-browse").and_then(|v| v.parse().ok()),
            scripted: SCRIPTED_FLAGS.iter().any(|f| has(f)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(s: &str) -> Cli {
        Cli::parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn a_race_line_reads_its_positionals_and_values() {
        let c = cli("alps 993 --autotest 8 --opponents 3 --record run.csv");
        assert_eq!(c.positional, ["alps", "993"]);
        assert_eq!((c.autotest_flag, c.autotest, c.opponents), (true, Some(8.0), Some(3)));
        assert_eq!(c.record.as_deref(), Some("run.csv"));
        assert!(c.scripted);
        assert!(!c.net_flag && c.net.is_none() && c.fe.is_none());
    }

    #[test]
    fn a_flag_value_is_never_positional_and_a_bare_flag_takes_none() {
        // `--follow` and `--mission` take no value: the token after them is positional
        let c = cli("--follow coastal --laps x --mission 911 --net");
        assert_eq!(c.positional, ["coastal", "911"]);
        assert_eq!(c.laps, None, "a value that does not parse is absent");
        assert!(c.follow && c.mission);
        assert!(c.net_flag && c.net.is_none(), "present without its value");
    }

    #[test]
    fn the_first_occurrence_wins_and_a_value_may_be_a_flag() {
        let c = cli("--laps 3 --laps 5 --screenshot --record");
        assert_eq!(c.laps, Some(3));
        assert_eq!(c.screenshot.as_deref(), Some("--record"));
        assert!(c.scripted);
    }

    #[test]
    fn fe_names_its_layout_or_the_quick_race() {
        assert_eq!(cli("--fe").fe.as_deref(), Some("QuickRace"));
        assert_eq!(cli("--fe=").fe.as_deref(), Some("QuickRace"));
        assert_eq!(cli("--fe=carcompare --fe=garage").fe.as_deref(), Some("carcompare"));
        let c = cli("--net join:127.0.0.1:9577 --net-name JOINY --net-browse 2.5");
        assert_eq!((c.net.as_deref(), c.net_name.as_deref(), c.net_browse), (Some("join:127.0.0.1:9577"), Some("JOINY"), Some(2.5)));
        assert!(c.positional.is_empty() && !c.scripted);
    }
}
