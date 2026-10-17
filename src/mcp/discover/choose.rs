//! Whether crowbot uses the servers other apps list: asked once per server before a session and
//! remembered in ~/.crowbot/mcp.json. A no is never asked again; a server whose program or
//! address changed is asked about again. A repository's own files are asked about apart, for
//! that project alone, defaulting to no.
// CEILING: one answer for every server a question lists. Upgrade: a card that ticks servers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Found, Scope, TEXT, found};
use crate::config::paths::Paths;
use crate::config::settings::Settings;
use crate::config::trust;
use crate::io;
use crate::mcp::config::{Reach, Server};
use crate::text::template::fill;

/// What the user chose, by server. Every field but the first is optional so older files load; a
/// file that does not load at all only means asking again.
#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Choice {
    scope: Scope,
    /// The project a repository's server was chosen for; none for the user's own apps.
    #[serde(default)]
    project: Option<String>,
    name: String,
    fingerprint: String,
    used: bool,
}

impl Choice {
    fn of(found: &Found, project: &str, used: bool) -> Self {
        Self {
            scope: found.scope,
            project: (found.scope == Scope::Project).then(|| project.to_owned()),
            name: found.name.clone(),
            fingerprint: found.fingerprint.clone(),
            used,
        }
    }

    /// Whether this is the choice about `found`, as it is now.
    fn about(&self, found: &Found, project: &str) -> bool {
        let same = Self::of(found, project, self.used);
        *self == same
    }

    /// Whether this is a choice about the same server's name and place, whatever it ran then.
    fn replaced_by(&self, other: &Self) -> bool {
        (self.scope, &self.project, &self.name) == (other.scope, &other.project, &other.name)
    }
}

/// What to do with what was found.
#[derive(Debug, Default)]
struct Plan {
    used: Vec<Found>,
    ask_user: Vec<Found>,
    ask_project: Vec<Found>,
}

/// Servers crowbot's own config names are its own; the rest are used, passed over or asked
/// about by what the user chose before.
fn plan(found: Vec<Found>, store: &Store, own: &BTreeMap<String, Server>, project: &str) -> Plan {
    let mut plan = Plan::default();
    for found in found.into_iter().filter(|f| !own.contains_key(&f.name)) {
        match store.choices.iter().find(|c| c.about(&found, project)) {
            Some(choice) if choice.used => plan.used.push(found),
            Some(_) => {}
            None if found.scope == Scope::User => plan.ask_user.push(found),
            None => plan.ask_project.push(found),
        }
    }
    plan
}

/// Finds the servers other apps list and adds those chosen to `settings.mcp`, asking about new
/// ones when `can_ask`; a run that cannot ask says what it found and leaves it out.
pub fn settle(paths: &Paths, settings: &mut Settings, can_ask: bool) -> anyhow::Result<()> {
    if !settings.mcp_discovery {
        return Ok(());
    }
    let (found, notes) = found(&paths.project, &|name| io::env::var(name));
    settings.found_notes.extend(notes);
    let project = paths.project.display().to_string();
    let file = paths.mcp_choices();
    let mut store: Store = io::fs::read_json(&file).ok().flatten().unwrap_or_default();
    let plan = plan(found, &store, &settings.mcp, &project);
    let mut used = plan.used;
    let mut chose = false;
    for (asked, scope) in [
        (plan.ask_user, Scope::User),
        (plan.ask_project, Scope::Project),
    ] {
        if asked.is_empty() {
            continue;
        }
        if !can_ask {
            for found in &asked {
                let sources = found.sources.join(", ");
                let note = fill(
                    &TEXT.unchosen,
                    &[("name", &found.name), ("sources", &sources)],
                );
                settings.found_notes.push((found.name.clone(), note));
            }
            continue;
        }
        let Some(yes) = ask(&asked, scope)? else {
            continue;
        };
        for found in &asked {
            let choice = Choice::of(found, &project, yes);
            store.choices.retain(|c| !c.replaced_by(&choice));
            store.choices.push(choice);
        }
        chose = true;
        if yes {
            used.extend(asked);
        }
    }
    if chose {
        io::fs::write_json(&file, &store, io::fs::Access::Private)?;
    }
    for found in used {
        let server = Server {
            reach: found.reach,
            enabled: true,
            timeout_secs: None,
            tools: None,
            from: Some(found.sources.join(", ")),
        };
        settings.mcp.insert(found.name, server);
    }
    Ok(())
}

/// Lists `asked` on the terminal and asks once for all of them; `None` when no answer came.
fn ask(asked: &[Found], scope: Scope) -> anyhow::Result<Option<bool>> {
    let (header, question, default) = match scope {
        Scope::User => (TEXT.found.clone(), &TEXT.question, true),
        Scope::Project => (
            format!("{}\n{}", TEXT.found_project, TEXT.danger),
            &TEXT.question_project,
            false,
        ),
    };
    let mut text = format!("{header}\n");
    for found in asked {
        text.push_str(&listed(found));
        text.push('\n');
    }
    io::term::out(&text);
    Ok(io::term::confirm(question, default)?)
}

/// One server as the question lists it: what it runs or reaches, its secrets by name only.
fn listed(found: &Found) -> String {
    let (what, secret_names): (String, Vec<&String>) = match &found.reach {
        Reach::Program { command, args, env } => {
            let mut line = vec![command.as_str()];
            line.extend(args.iter().map(String::as_str));
            (line.join(" "), env.keys().collect())
        }
        Reach::Url { url, headers } => (url.clone(), headers.keys().collect()),
    };
    let secrets = match secret_names.is_empty() {
        true => String::new(),
        false => {
            let names: Vec<String> = secret_names
                .iter()
                .map(|n| format!("{n}={}", trust::mask()))
                .collect();
            fill(&TEXT.secrets, &[("names", &names.join(", "))])
        }
    };
    let sources = found.sources.join(", ");
    fill(
        &TEXT.entry,
        &[
            ("name", &found.name),
            ("what", &what),
            ("secrets", &secrets),
            ("sources", &sources),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, scope: Scope, fingerprint: &str) -> Found {
        Found {
            name: name.into(),
            reach: Reach::Program {
                command: "tool".into(),
                args: vec!["--stdio".into()],
                env: BTreeMap::from([("TOKEN".to_owned(), "s3cret".to_owned())]),
            },
            scope,
            sources: vec!["Cursor".into()],
            fingerprint: fingerprint.into(),
        }
    }

    fn names(found: &[Found]) -> Vec<&str> {
        found.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn what_was_chosen_holds_until_the_server_changes_and_crowbots_own_config_wins() {
        let project = "/p";
        let yes = Choice::of(&found("kept", Scope::User, "f1"), project, true);
        let no = Choice::of(&found("refused", Scope::User, "f1"), project, false);
        let old = Choice::of(&found("changed", Scope::User, "f1"), project, true);
        let elsewhere = Choice::of(&found("repo", Scope::Project, "f1"), "/other", true);
        let store = Store {
            choices: vec![yes, no, old, elsewhere],
        };
        let own: BTreeMap<String, Server> = toml::from_str::<toml::Table>("command = 'mine'")
            .map(|t| {
                BTreeMap::from([("ours".to_owned(), toml::Value::Table(t).try_into().unwrap())])
            })
            .unwrap();
        let plan = plan(
            vec![
                found("kept", Scope::User, "f1"),
                found("refused", Scope::User, "f1"),
                found("changed", Scope::User, "f2"),
                found("new", Scope::User, "f1"),
                found("ours", Scope::User, "f1"),
                found("repo", Scope::Project, "f1"),
            ],
            &store,
            &own,
            project,
        );
        assert_eq!(names(&plan.used), ["kept"]);
        assert_eq!(names(&plan.ask_user), ["changed", "new"]);
        // Chosen for another project, so asked about again here.
        assert_eq!(names(&plan.ask_project), ["repo"]);
    }

    #[test]
    fn a_new_choice_replaces_the_old_one_for_that_server() {
        let old = Choice::of(&found("x", Scope::User, "f1"), "/p", false);
        let new = Choice::of(&found("x", Scope::User, "f2"), "/p", true);
        assert!(old.replaced_by(&new));
        let other = Choice::of(&found("y", Scope::User, "f1"), "/p", true);
        assert!(!other.replaced_by(&new));
    }

    #[test]
    fn a_listed_server_shows_what_it_runs_and_its_secrets_by_name_only() {
        let line = listed(&found("ida", Scope::User, "f1"));
        assert!(line.contains("ida: tool --stdio"), "{line}");
        assert!(
            line.contains("TOKEN=") && !line.contains("s3cret"),
            "{line}"
        );
        assert!(line.ends_with("(Cursor)"), "{line}");
    }
}
