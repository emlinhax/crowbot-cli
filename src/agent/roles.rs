//! Which model and effort a subagent runs on: the `task` call's own pick, else its kind's, else
//! every kind's, else the chat's. Model and effort inherit separately. One resolver for every
//! launch, so what `/models` shows is what a subagent is sent.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::agent::kinds;
use crate::api::models::Model;
use crate::config::subagents::Subagents;
use crate::data;
use crate::models::Catalog;
use crate::text::template::fill;

static TEXT: LazyLock<Text> = data::load!("roles.toml");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    chat: String,
    every: String,
    unknown_model: String,
    not_allowed: String,
    from: From,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct From {
    call: String,
    kind: String,
    agent: String,
    subagents: String,
    chat: String,
}

/// Something that runs on a model of its own: the chat, every kind of subagent, or one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleId {
    Chat,
    Every,
    Kind(String),
}

impl RoleId {
    /// The chat, every kind, then each kind in data/agents/ order.
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::Chat, Self::Every];
        all.extend(kinds::all().iter().map(|k| Self::Kind(k.id.clone())));
        all
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|r| r.id() == id)
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Chat => &TEXT.chat,
            Self::Every => &TEXT.every,
            Self::Kind(id) => id,
        }
    }

    /// The `Subagents` entry it is: `Some(None)` for every kind, `None` for the chat.
    pub fn kind(&self) -> Option<Option<&str>> {
        match self {
            Self::Chat => None,
            Self::Every => Some(None),
            Self::Kind(id) => Some(Some(id)),
        }
    }
}

/// Where a resolved choice came from: the call, the user's settings for the kind, the kind's
/// own agent file, the settings for every kind, or the chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Call,
    Kind,
    /// The kind's agent file (agent/agent_file.rs).
    Agent,
    Subagents,
    Chat,
}

impl Source {
    pub fn label(self, kind: &str) -> String {
        let from = &TEXT.from;
        match self {
            Self::Call => from.call.clone(),
            Self::Kind => fill(&from.kind, &[("kind", kind)]),
            Self::Agent => fill(&from.agent, &[("kind", kind)]),
            Self::Subagents => from.subagents.clone(),
            Self::Chat => from.chat.clone(),
        }
    }
}

/// The chat's choice, which a subagent inherits last.
pub struct Chat<'a> {
    pub model: &'a Model,
    pub effort: Option<&'a str>,
}

pub struct Resolved {
    pub model: Model,
    /// What is asked for: `None` for a model that does not reason.
    pub effort: Option<String>,
    pub source: Source,
}

/// The model id a subagent of `kind` would run on and where it came from, before the catalog is
/// asked: what `/models` shows, even for a model crowbot no longer serves.
pub fn chosen<'a>(
    kind: Option<&str>,
    asked: Option<&'a str>,
    subagents: &'a Subagents,
    chat: &'a str,
) -> (&'a str, Source) {
    let own = kind.and_then(|id| subagents.kinds.get(id)?.model.as_deref());
    let file = kind.and_then(|id| kinds::get(id)?.model.as_deref());
    asked
        .map(|m| (m, Source::Call))
        .or(own.map(|m| (m, Source::Kind)))
        .or(file.map(|m| (m, Source::Agent)))
        .or(subagents.model.as_deref().map(|m| (m, Source::Subagents)))
        .unwrap_or((chat, Source::Chat))
}

/// The effort asked of a subagent of `kind` before its model says whether it reasons.
pub fn chosen_effort<'a>(
    kind: Option<&str>,
    subagents: &'a Subagents,
    chat: Option<&'a str>,
) -> Option<&'a str> {
    kind.and_then(|id| subagents.kinds.get(id)?.effort.as_deref())
        .or(kind.and_then(|id| kinds::get(id)?.effort.as_deref()))
        .or(subagents.effort.as_deref())
        .or(chat)
}

/// What a subagent of `kind` runs on. `Err` says why, and nothing falls back silently: a model
/// the user or the call named that crowbot does not serve, or that `choices` leaves out, fails
/// the call.
pub fn resolve(
    kind: &str,
    asked: Option<&str>,
    subagents: &Subagents,
    catalog: &Catalog,
    chat: &Chat<'_>,
) -> Result<Resolved, String> {
    let (id, source) = chosen(Some(kind), asked, subagents, &chat.model.id);
    if source == Source::Call
        && let Some(choices) = &subagents.choices
        && !choices.iter().any(|c| c == id)
    {
        let own = chosen(Some(kind), None, subagents, &chat.model.id).0;
        return Err(fill(
            &TEXT.not_allowed,
            &[
                ("model", id),
                ("choices", &choices.join(", ")),
                ("kind", &format!("{kind} ({own})")),
            ],
        ));
    }
    let model = match source {
        // The chat's model is the one already in use, whatever the catalog says of it now.
        Source::Chat => chat.model.clone(),
        _ => catalog.get(id).cloned().ok_or_else(|| {
            fill(
                &TEXT.unknown_model,
                &[("model", id), ("from", &source.label(kind))],
            )
        })?,
    };
    let effort = chosen_effort(Some(kind), subagents, chat.effort);
    let effort = model.effort(effort).map(str::to_owned);
    Ok(Resolved {
        model,
        effort,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::subagents::Role;
    use crate::models::Source as Origin;

    fn catalog() -> Catalog {
        Catalog {
            models: crate::models::snapshot(),
            source: Origin::Snapshot,
            fetched_at: None,
            note: None,
        }
    }

    fn reasoning(catalog: &Catalog) -> Model {
        let model = catalog.models.iter().find(|m| m.capabilities.reasoning);
        model.expect("a reasoning model in the snapshot").clone()
    }

    fn ids(catalog: &Catalog) -> Vec<String> {
        catalog.models.iter().map(|m| m.id.clone()).collect()
    }

    fn role(model: Option<&str>, effort: Option<&str>) -> Role {
        Role {
            model: model.map(Into::into),
            effort: effort.map(Into::into),
        }
    }

    #[test]
    fn roles_are_the_chat_every_kind_and_each_kind_by_id() {
        let ids: Vec<String> = RoleId::all().iter().map(|r| r.id().to_owned()).collect();
        assert_eq!(ids, ["chat", "subagents", "explore", "general"]);
        assert_eq!(
            RoleId::parse("explore").unwrap().kind(),
            Some(Some("explore"))
        );
        assert_eq!(RoleId::parse("subagents").unwrap().kind(), Some(None));
        assert_eq!(RoleId::parse("chat").unwrap().kind(), None);
        assert!(RoleId::parse("nope").is_none());
    }

    #[test]
    fn the_call_beats_the_kind_beats_every_kind_beats_the_chat() {
        let catalog = catalog();
        let ids = ids(&catalog);
        let chat_model = reasoning(&catalog);
        let chat = Chat {
            model: &chat_model,
            effort: None,
        };
        let mut s = Subagents::default();
        let got = |s: &Subagents, asked: Option<&str>| {
            let r = resolve("explore", asked, s, &catalog, &chat).unwrap();
            (r.model.id, r.source)
        };
        assert_eq!(got(&s, None), (chat_model.id.clone(), Source::Chat));
        s.model = Some(ids[1].clone());
        assert_eq!(got(&s, None), (ids[1].clone(), Source::Subagents));
        s.set(Some("explore"), role(Some(&ids[2]), None));
        assert_eq!(got(&s, None), (ids[2].clone(), Source::Kind));
        // Another kind still has only every kind's.
        let general = resolve("general", None, &s, &catalog, &chat).unwrap();
        assert_eq!(general.model.id, ids[1]);
        assert_eq!(got(&s, Some(&ids[3])), (ids[3].clone(), Source::Call));
    }

    #[test]
    fn effort_inherits_on_its_own_chain_and_a_model_that_does_not_reason_drops_it() {
        let catalog = catalog();
        let chat_model = reasoning(&catalog);
        let levels = crate::effort::ids();
        let chat = Chat {
            model: &chat_model,
            effort: Some(levels[0]),
        };
        let mut s = Subagents::default();
        let effort = |s: &Subagents| resolve("explore", None, s, &catalog, &chat).unwrap().effort;
        assert_eq!(effort(&s).as_deref(), Some(levels[0]));
        s.effort = Some(levels[1].into());
        assert_eq!(effort(&s).as_deref(), Some(levels[1]));
        s.set(Some("explore"), role(None, Some(levels[2])));
        assert_eq!(effort(&s).as_deref(), Some(levels[2]));

        if let Some(plain) = catalog.models.iter().find(|m| !m.capabilities.reasoning) {
            s.set(Some("explore"), role(Some(&plain.id), Some(levels[2])));
            assert_eq!(effort(&s), None);
        }
    }

    #[test]
    fn an_unknown_model_fails_naming_where_it_came_from_and_never_falls_back() {
        let catalog = catalog();
        let chat_model = reasoning(&catalog);
        let chat = Chat {
            model: &chat_model,
            effort: None,
        };
        let mut s = Subagents::default();
        s.set(Some("explore"), role(Some("gone-model"), None));
        let err = resolve("explore", None, &s, &catalog, &chat).err().unwrap();
        assert!(
            err.contains("gone-model") && err.contains("subagents.kinds.explore"),
            "{err}"
        );
        let err = resolve("general", Some("typo"), &s, &catalog, &chat)
            .err()
            .unwrap();
        assert!(err.contains("typo") && err.contains("task call"), "{err}");
    }

    #[test]
    fn choices_limit_what_a_call_may_name_but_not_what_the_user_set() {
        let catalog = catalog();
        let ids = ids(&catalog);
        let chat_model = reasoning(&catalog);
        let chat = Chat {
            model: &chat_model,
            effort: None,
        };
        let mut s = Subagents {
            choices: Some(vec![ids[0].clone()]),
            ..Subagents::default()
        };
        s.set(Some("explore"), role(Some(&ids[1]), None));
        assert!(resolve("explore", Some(&ids[0]), &s, &catalog, &chat).is_ok());
        let err = resolve("explore", Some(&ids[2]), &s, &catalog, &chat)
            .err()
            .unwrap();
        assert!(err.contains(&ids[2]) && err.contains(&ids[1]), "{err}");
        assert_eq!(
            resolve("explore", None, &s, &catalog, &chat)
                .unwrap()
                .model
                .id,
            ids[1]
        );
    }
}
