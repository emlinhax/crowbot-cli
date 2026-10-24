//! Presets: files in ~/.crowbot/presets/ that a session picks like a model. Each request then runs
//! on the model and effort of the first rule whose conditions all hold, so one conversation can
//! send cheap work to a cheap model and hard work to a strong one.
// CEILING: the user's folder only. Upgrade: a trusted project's .crowbot/presets/, listed in the
// trust prompt beside its settings.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;

use crate::agent::kinds;
use crate::api::account::Me;
use crate::api::models::Model;
use crate::config::paths::Paths;
use crate::models::Catalog;
use crate::text::choices;
use crate::text::template::fill;
use crate::{data, effort, io, mode};

static TEXT: LazyLock<Text> = data::load!("presets.toml");
const TEMPLATE: &str = data::raw!("preset_template.toml");
const EXTENSION: &str = "toml";

/// The `agent` values that are not kinds: the agent the user works with, and any subagent.
const MAIN: &str = "main";
const SUBAGENT: &str = "subagent";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    broken: String,
    unreadable: String,
    no_rules: String,
    last_has_when: String,
    dead_rule: String,
    in_rule: String,
    unknown_model: String,
    shadowed: String,
    bad_name: String,
    exists: String,
}

/// One preset file as read, before its models are looked up.
#[derive(Clone, Debug)]
pub struct Preset {
    /// The file's name without `.toml`.
    pub id: String,
    pub path: PathBuf,
    pub description: String,
    pub rules: Vec<Rule>,
    /// Why the file cannot be used; such a preset is listed, but choosing it is refused.
    pub problem: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    description: String,
    #[serde(default)]
    rule: Vec<Rule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    #[serde(default)]
    pub when: When,
    pub model: String,
    /// `None` keeps the session's effort.
    #[serde(default)]
    pub effort: Option<String>,
}

/// What must hold for a rule to apply: every condition it sets. One that sets none always holds.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// Mode ids (data/modes/).
    #[serde(default)]
    pub mode: Vec<String>,
    /// `main`, `subagent`, or a kind of subagent (data/agents/).
    #[serde(default)]
    pub agent: Vec<String>,
    /// The last request sent more tokens than this.
    #[serde(default)]
    pub context_over: Option<u64>,
    /// The plan's allowance left is under this share, in percent.
    #[serde(default)]
    pub allowance_under_pct: Option<u32>,
    #[serde(default)]
    pub balance_under_usd: Option<f64>,
}

/// What a request is about to be sent under, as a rule reads it.
pub struct Signals<'a> {
    pub mode: &'a str,
    /// The kind of subagent sending it; `None` for the agent the user works with.
    pub agent: Option<&'a str>,
    /// Tokens the last request sent, when one has since the last summary.
    pub context: Option<u64>,
    /// Where the account stood when last asked; `None` when it has not been.
    pub account: Option<&'a Me>,
}

impl When {
    fn always(&self) -> bool {
        *self == Self::default()
    }

    /// Whether every condition set holds; one whose signal is not known does not.
    fn holds(&self, s: &Signals<'_>) -> bool {
        let mode = self.mode.is_empty() || self.mode.iter().any(|m| m == s.mode);
        let agent = self.agent.is_empty()
            || self.agent.iter().any(|a| match (a.as_str(), s.agent) {
                (MAIN, None) | (SUBAGENT, Some(_)) => true,
                (kind, Some(sent_by)) => kind == sent_by,
                (_, None) => false,
            });
        let context = self
            .context_over
            .is_none_or(|over| s.context.is_some_and(|sent| sent > over));
        let allowance = self.allowance_under_pct.is_none_or(|under| {
            let pass = s.account.and_then(|me| me.pass.as_ref());
            pass.is_some_and(|p| p.allowance_bp < under.saturating_mul(100))
        });
        let balance = self.balance_under_usd.is_none_or(|under| {
            s.account
                .is_some_and(|me| (me.balance_microdollars as f64) < under * 1e6)
        });
        mode && agent && context && allowance && balance
    }

    /// Every id it names is one crowbot knows.
    fn check(&self) -> anyhow::Result<()> {
        for id in &self.mode {
            mode::find(id)?;
        }
        for id in &self.agent {
            if id != MAIN && id != SUBAGENT && kinds::get(id).is_none() {
                return Err(choices::unknown("agent", id, agents()));
            }
        }
        Ok(())
    }
}

/// What `agent` may name, as an unknown one is told.
fn agents() -> impl Iterator<Item = (&'static str, Option<String>)> {
    [MAIN, SUBAGENT]
        .into_iter()
        .chain(kinds::all().iter().map(|k| k.id.as_str()))
        .map(|id| (id, None))
}

/// Every preset in the user's folder, by name; a file that does not load is listed with why.
pub fn load(paths: &Paths) -> Vec<Preset> {
    let dir = paths.presets();
    let Ok(entries) = io::fs::list_dir(&dir) else {
        return Vec::new();
    };
    let mut presets: Vec<Preset> = entries
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            let path = dir.join(&e.name);
            let id = e.name.strip_suffix(&format!(".{EXTENSION}"))?.to_owned();
            Some(read(id, path))
        })
        .collect();
    presets.sort_by(|a, b| a.id.cmp(&b.id));
    presets
}

fn read(id: String, path: PathBuf) -> Preset {
    let text = io::fs::read_string(&path)
        .map_err(|e| fill(&TEXT.unreadable, &[("error", &e.to_string())]))
        .map(Option::unwrap_or_default);
    from_text(id, path, text)
}

/// A preset from what its file held, or why it could not be read.
fn from_text(id: String, path: PathBuf, text: Result<String, String>) -> Preset {
    let parsed = text.and_then(|text| parse(&text).map_err(|e| format!("{e:#}")));
    let (description, rules, problem) = match parsed {
        Ok(file) => (file.description, file.rule, None),
        Err(problem) => (String::new(), Vec::new(), Some(problem)),
    };
    Preset {
        id,
        path,
        description,
        rules,
        problem,
    }
}

/// A preset named `id` with this text, as though read from the user's folder.
#[cfg(test)]
pub fn parsed(id: &str, text: &str) -> Preset {
    let path = PathBuf::from(format!("/home/u/.crowbot/presets/{id}.{EXTENSION}"));
    from_text(id.to_owned(), path, Ok(text.to_owned()))
}

/// A preset file's text, checked: the last rule takes every request no other rule takes, and
/// every mode, agent and effort it names exists.
fn parse(text: &str) -> anyhow::Result<File> {
    let file: File = toml::from_str(text)?;
    let Some((last, before)) = file.rule.split_last() else {
        bail!("{}", TEXT.no_rules);
    };
    if !last.when.always() {
        bail!("{}", TEXT.last_has_when);
    }
    if let Some(n) = before.iter().position(|r| r.when.always()) {
        bail!("{}", fill(&TEXT.dead_rule, &[("n", &(n + 1).to_string())]));
    }
    for (i, rule) in file.rule.iter().enumerate() {
        let at = || fill(&TEXT.in_rule, &[("n", &(i + 1).to_string())]);
        rule.when.check().with_context(at)?;
        if let Some(level) = &rule.effort {
            effort::find(level).with_context(at)?;
        }
    }
    Ok(file)
}

/// A preset with its models looked up, ready to pick from.
#[derive(Clone, Debug)]
pub struct Routes {
    pub id: String,
    pub path: PathBuf,
    /// Never empty, and the last one always holds (`parse`).
    routes: Vec<Route>,
}

#[derive(Clone, Debug)]
struct Route {
    when: When,
    model: Model,
    effort: Option<String>,
}

impl Preset {
    /// Its rules with their models from `catalog`; `Err` says what keeps it from being chosen.
    pub fn resolve(&self, catalog: &Catalog) -> anyhow::Result<Routes> {
        self.usable(catalog).map_err(|problem| {
            let path = self.path.display().to_string();
            anyhow!(fill(
                &TEXT.broken,
                &[("id", &self.id), ("path", &path), ("problem", &problem)],
            ))
        })
    }

    fn usable(&self, catalog: &Catalog) -> Result<Routes, String> {
        if let Some(problem) = &self.problem {
            return Err(problem.clone());
        }
        if catalog.get(&self.id).is_some() {
            return Err(TEXT.shadowed.clone());
        }
        let routes = self
            .rules
            .iter()
            .enumerate()
            .map(|(i, rule)| {
                let model = catalog.get(&rule.model).cloned().ok_or_else(|| {
                    let n = (i + 1).to_string();
                    fill(&TEXT.unknown_model, &[("n", &n), ("model", &rule.model)])
                })?;
                Ok(Route {
                    when: rule.when.clone(),
                    model,
                    effort: rule.effort.clone(),
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Routes {
            id: self.id.clone(),
            path: self.path.clone(),
            routes,
        })
    }
}

impl Routes {
    /// The model and effort of the first rule that holds; effort `None` keeps the session's.
    pub fn pick(&self, signals: &Signals<'_>) -> (&Model, Option<&str>) {
        let route = self
            .routes
            .iter()
            .find(|r| r.when.holds(signals))
            .unwrap_or_else(|| self.last());
        (&route.model, route.effort.as_deref())
    }

    /// Whether a rule asks where the account stands, so a run must find out.
    pub fn reads_account(&self) -> bool {
        self.routes
            .iter()
            .any(|r| r.when.allowance_under_pct.is_some() || r.when.balance_under_usd.is_some())
    }

    /// The model of the rule that takes what no other does.
    pub fn fallback(&self) -> &Model {
        &self.last().model
    }

    fn last(&self) -> &Route {
        self.routes
            .last()
            .expect("a preset has a rule; parse checks")
    }
}

/// Writes a new preset from the template, its rules on `model`, named `name` or else the first
/// free `new`, `new-2`, …; returns where.
pub fn create(paths: &Paths, name: Option<&str>, model: &str) -> anyhow::Result<PathBuf> {
    let dir = paths.presets();
    let path = match name {
        Some(name) => {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                bail!("{}", fill(&TEXT.bad_name, &[("name", name)]));
            }
            let path = file(&dir, name);
            if io::fs::exists(&path) {
                bail!(
                    "{}",
                    fill(&TEXT.exists, &[("path", &path.display().to_string())])
                );
            }
            path
        }
        None => (1..)
            .map(|n| match n {
                1 => file(&dir, "new"),
                n => file(&dir, &format!("new-{n}")),
            })
            .find(|path| !io::fs::exists(path))
            .expect("some name is free"),
    };
    let id = path.file_stem().unwrap_or_default().to_string_lossy();
    io::fs::write_atomic(
        &path,
        template(&id, model).as_bytes(),
        io::fs::Access::Shared,
    )?;
    Ok(path)
}

fn file(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{EXTENSION}"))
}

/// The template filled in: what to pick it by, every mode, kind and effort, and `model`.
fn template(id: &str, model: &str) -> String {
    let kinds: Vec<&str> = kinds::all().iter().map(|k| k.id.as_str()).collect();
    fill(
        TEMPLATE,
        &[
            ("id", id),
            ("model", model),
            ("modes", &mode::ids().join(", ")),
            ("kinds", &kinds.join(", ")),
            ("efforts", &effort::ids().join(", ")),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::account::Pass;
    use crate::models::{self as catalogs, Source};

    fn catalog() -> Catalog {
        Catalog {
            models: catalogs::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        }
    }

    fn model(i: usize) -> String {
        catalogs::snapshot()[i].id.clone()
    }

    fn preset(text: &str) -> Preset {
        parsed("p", text)
    }

    fn signals<'a>(mode: &'a str, agent: Option<&'a str>) -> Signals<'a> {
        Signals {
            mode,
            agent,
            context: None,
            account: None,
        }
    }

    fn routes(text: &str) -> Routes {
        preset(text).resolve(&catalog()).unwrap()
    }

    #[test]
    fn the_first_rule_that_holds_picks_and_the_last_takes_the_rest() {
        let (a, b, c) = (model(0), model(1), model(2));
        let routes = routes(&format!(
            r#"
            [[rule]]
            when = {{ agent = ["subagent"] }}
            model = "{a}"
            effort = "low"
            [[rule]]
            when = {{ mode = ["plan"], agent = ["main"] }}
            model = "{b}"
            [[rule]]
            model = "{c}"
            "#
        ));
        let picked = |s: Signals<'_>| {
            let (m, e) = routes.pick(&s);
            (m.id.clone(), e.map(str::to_owned))
        };
        assert_eq!(
            picked(signals("plan", Some("explore"))),
            (a, Some("low".into()))
        );
        assert_eq!(picked(signals("plan", None)), (b, None));
        assert_eq!(picked(signals("auto", None)), (c.clone(), None));
        assert_eq!(routes.fallback().id, c);
    }

    #[test]
    fn a_kind_matches_only_its_own_subagents_and_main_only_the_session() {
        let when = |agent: &str| When {
            agent: vec![agent.into()],
            ..When::default()
        };
        assert!(when("explore").holds(&signals("auto", Some("explore"))));
        assert!(!when("explore").holds(&signals("auto", Some("general"))));
        assert!(!when("explore").holds(&signals("auto", None)));
        assert!(when("main").holds(&signals("auto", None)));
        assert!(!when("main").holds(&signals("auto", Some("general"))));
    }

    #[test]
    fn a_condition_whose_signal_is_unknown_does_not_hold() {
        let context = When {
            context_over: Some(1000),
            ..When::default()
        };
        let allowance = When {
            allowance_under_pct: Some(20),
            ..When::default()
        };
        let balance = When {
            balance_under_usd: Some(5.0),
            ..When::default()
        };
        let none = signals("auto", None);
        assert!(!context.holds(&none) && !allowance.holds(&none) && !balance.holds(&none));

        let low = Me {
            balance_microdollars: 4_000_000,
            pass: Some(Pass {
                allowance_bp: 1_999,
                ..Pass::default()
            }),
            ..Me::default()
        };
        let known = Signals {
            context: Some(1001),
            account: Some(&low),
            ..none
        };
        assert!(context.holds(&known) && allowance.holds(&known) && balance.holds(&known));

        // An account with no plan has no allowance to be under.
        let no_plan = Me::default();
        let planless = Signals {
            account: Some(&no_plan),
            ..signals("auto", None)
        };
        assert!(!allowance.holds(&planless));
    }

    #[test]
    fn a_file_that_cannot_pick_for_every_request_is_listed_with_why() {
        let m = model(0);
        let cases = [
            ("description = \"x\"".to_owned(), "no [[rule]]"),
            (
                format!("[[rule]]\nwhen = {{ mode = [\"plan\"] }}\nmodel = \"{m}\""),
                "last rule has a `when`",
            ),
            (
                format!("[[rule]]\nmodel = \"{m}\"\n[[rule]]\nmodel = \"{m}\""),
                "rule 1 has no `when`",
            ),
            (
                format!(
                    "[[rule]]\nwhen = {{ agent = [\"reviewer\"] }}\nmodel = \"{m}\"\n[[rule]]\nmodel = \"{m}\""
                ),
                "rule 1: unknown agent `reviewer`",
            ),
            (
                format!("[[rule]]\nmodel = \"{m}\"\neffort = \"ludicrous\""),
                "rule 1: unknown effort",
            ),
            (format!("[[rule]]\nmodel = \"{m}\"\nspeed = 3"), "speed"),
        ];
        for (text, said) in cases {
            let p = preset(&text);
            let problem = p.problem.clone().unwrap_or_default();
            assert!(problem.contains(said), "{text}: {problem}");
            let refused = p.resolve(&catalog()).unwrap_err().to_string();
            assert!(
                refused.contains("preset `p`") && refused.contains(said),
                "{refused}"
            );
        }
    }

    #[test]
    fn an_unknown_model_or_a_models_name_refuses_the_choice() {
        let err = preset("[[rule]]\nmodel = \"nope\"")
            .resolve(&catalog())
            .unwrap_err();
        assert!(
            err.to_string().contains("rule 1 names model `nope`"),
            "{err}"
        );
        let mut shadowing = preset(&format!("[[rule]]\nmodel = \"{}\"", model(0)));
        shadowing.id = model(1);
        let err = shadowing.resolve(&catalog()).unwrap_err();
        assert!(err.to_string().contains("same name"), "{err}");
    }

    #[test]
    fn a_new_preset_is_the_template_on_the_model_given_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        let m = model(0);
        let first = create(&paths, None, &m).unwrap();
        let second = create(&paths, None, &m).unwrap();
        assert!(first.ends_with("presets/new.toml"), "{first:?}");
        assert!(second.ends_with("presets/new-2.toml"), "{second:?}");
        create(&paths, Some("cheap"), &m).unwrap();
        assert!(create(&paths, Some("cheap"), &m).is_err());
        assert!(create(&paths, Some("../up"), &m).is_err());

        let listed = load(&paths);
        let ids: Vec<&str> = listed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["cheap", "new", "new-2"]);
        for p in &listed {
            assert!(p.problem.is_none(), "{:?}", p.problem);
            assert_eq!(p.resolve(&catalog()).unwrap().fallback().id, m);
        }
        let text = io::fs::read_string(&first).unwrap().unwrap();
        assert!(text.contains("--model new"), "{text}");
        assert!(
            !text.contains("{kinds}") && text.contains("explore"),
            "{text}"
        );
    }
}
