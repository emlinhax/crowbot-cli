//! `/models` in a session: what the chat and each kind of subagent run on, one row each. A pick
//! opens the model list (the chat's switches the session as before); for a subagent, its effort
//! after, then the choice is kept in prefs.json. Esc steps back a list without changing anything.

use crate::agent::roles::{self, RoleId, Source};
use crate::config::subagents::{Role, Subagents};
use crate::effort;
use crate::frontend::tui::card::{Card, Next};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::picker::{Chose, Pick, Picker, Table};
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::models::Catalog;
use crate::text::styled::{Line, Style};
use crate::text::table::Align;
use crate::text::template::fill;
use crate::text::units;

/// What the roles are set to as the card opens.
pub struct Now<'a> {
    pub catalog: &'a Catalog,
    pub chat: &'a str,
    /// The effort asked of the chat's model.
    pub chat_effort: Option<&'a str>,
    /// Every layer's choices, merged.
    pub subagents: &'a Subagents,
    /// The ones picked here before (prefs.json), to tell them from the config files'.
    pub picked: &'a Subagents,
    /// Where picks are kept, for the note under the list.
    pub kept_in: String,
}

pub struct Roles {
    catalog: Catalog,
    chat: String,
    chat_effort: Option<String>,
    subagents: Subagents,
    list: Picker,
    stage: Stage,
}

enum Stage {
    List,
    Model(RoleId, Picker),
    /// The model picked for the role (`None` inherits), and its efforts.
    Effort(RoleId, Option<String>, Picker),
}

impl Roles {
    /// The roles list, or straight at `role`'s model list.
    pub fn open(now: &Now<'_>, role: Option<RoleId>) -> Self {
        let words = &ui::get().roles;
        let all = RoleId::all();
        let rows = all.iter().map(|r| row(now, r)).collect();
        let list = Picker::table(
            all.iter().cloned().map(Pick::Role).collect(),
            table(rows),
            0,
            (words.title.clone(), words.hint.clone()),
            Some(fill(&words.note, &[("path", &now.kept_in)])),
        );
        let mut card = Self {
            catalog: now.catalog.clone(),
            chat: now.chat.to_owned(),
            chat_effort: now.chat_effort.map(str::to_owned),
            subagents: now.subagents.clone(),
            list,
            stage: Stage::List,
        };
        if let Some(role) = role {
            card.stage = card.models(role);
        }
        card
    }

    fn models(&self, role: RoleId) -> Stage {
        let words = &ui::get().roles;
        let picker = match role.kind() {
            None => Picker::models(&self.catalog, &self.chat),
            Some(kind) => {
                let own = self.subagents.role(kind).model;
                let base = self.without(kind, true);
                let inherited = roles::chosen(kind, None, &base, &self.chat).0;
                let inherit = fill(&words.inherit_model, &[("model", inherited)]);
                Picker::models_for(&self.catalog, own.as_deref(), Some(inherit))
            }
        };
        let title = fill(&words.model_title, &[("role", role.id())]);
        Stage::Model(role, picker.titled(title, words.step_hint.clone()))
    }

    fn efforts(&self, role: RoleId, model: Option<String>) -> Stage {
        let words = &ui::get().roles;
        let kind = role.kind().flatten();
        let own = self.subagents.role(role.kind().unwrap_or_default()).effort;
        let base = self.without(role.kind().unwrap_or_default(), false);
        let inherited = roles::chosen_effort(kind, &base, self.chat_effort.as_deref());
        let inherited = inherited.unwrap_or(&words.default_effort);
        let mut picks = vec![Pick::Effort(None)];
        let mut rows = vec![
            vec![Line::styled(&words.columns[2], Style::fg("muted").bold())],
            vec![Line::styled(
                fill(&words.inherit_effort, &[("effort", inherited)]),
                Style::fg("muted"),
            )],
        ];
        for level in effort::levels() {
            picks.push(Pick::Effort(Some(level.id.clone())));
            rows.push(vec![Line::plain(format!(
                "{}  {}",
                level.id, level.summary
            ))]);
        }
        let selected = own
            .and_then(|e| {
                picks
                    .iter()
                    .position(|p| *p == Pick::Effort(Some(e.clone())))
            })
            .unwrap_or(0);
        let table = Table {
            rows,
            aligns: vec![Align::Left],
            drop: Vec::new(),
        };
        let title = fill(&words.effort_title, &[("role", role.id())]);
        let picker = Picker::table(
            picks,
            table,
            selected,
            (title, words.step_hint.clone()),
            None,
        );
        Stage::Effort(role, model, picker)
    }

    /// The choices with `kind`'s own model (or effort) taken out: what it would inherit.
    fn without(&self, kind: Option<&str>, model: bool) -> Subagents {
        let mut base = self.subagents.clone();
        let mut own = base.role(kind);
        match model {
            true => own.model = None,
            false => own.effort = None,
        }
        base.set(kind, own);
        base
    }
}

impl Card for Roles {
    fn key(&mut self, action: Option<Action>, _key: &KeyEvent) -> Next {
        let stage = std::mem::replace(&mut self.stage, Stage::List);
        self.stage = match stage {
            Stage::List => match self.list.choose(action) {
                Chose::Picked(Pick::Role(role)) => self.models(role),
                Chose::Back => return Next::Close(None),
                _ => Stage::List,
            },
            Stage::Model(role, mut picker) => match (picker.choose(action), role.kind()) {
                (Chose::Picked(Pick::Model(id)), None) => return Next::UseModel(id),
                (Chose::Picked(Pick::Model(id)), Some(_)) => self.efforts(role, Some(id)),
                (Chose::Picked(Pick::Inherit), Some(_)) => self.efforts(role, None),
                (Chose::Back, _) => Stage::List,
                _ => Stage::Model(role, picker),
            },
            Stage::Effort(role, model, mut picker) => match picker.choose(action) {
                Chose::Picked(Pick::Effort(effort)) => {
                    return Next::Assign {
                        kind: role.kind().flatten().map(str::to_owned),
                        role: Role { model, effort },
                    };
                }
                Chose::Back => Stage::List,
                _ => Stage::Effort(role, model, picker),
            },
        };
        Next::Stay
    }

    fn render(&self, width: usize) -> Vec<Line> {
        match &self.stage {
            Stage::List => self.list.render(width),
            Stage::Model(_, picker) | Stage::Effort(_, _, picker) => picker.render(width),
        }
    }
}

/// What `role` runs on: its label, model, effort, where the model came from, and its prices.
fn row(now: &Now<'_>, role: &RoleId) -> Vec<Line> {
    let words = &ui::get().roles;
    let (label, model, effort, from) = match role.kind() {
        None => (words.chat.clone(), now.chat, now.chat_effort, String::new()),
        Some(kind) => {
            let (model, source) = roles::chosen(kind, None, now.subagents, now.chat);
            let effort = roles::chosen_effort(kind, now.subagents, now.chat_effort);
            let label = match kind {
                None => words.every.clone(),
                Some(id) => fill(&words.kind, &[("kind", id)]),
            };
            (label, model, effort, from(now, kind, source))
        }
    };
    let served = now.catalog.get(model);
    let model_cell = match served {
        Some(_) => Line::plain(model),
        None => Line::styled(
            fill(&words.unavailable, &[("model", model)]),
            Style::fg("error"),
        ),
    };
    let effort = match (served, effort) {
        (Some(m), _) if !m.capabilities.reasoning => words.no_effort.clone(),
        (_, Some(e)) => e.to_owned(),
        (_, None) => words.default_effort.clone(),
    };
    let price = |usd: Option<f64>| usd.map(units::usd).unwrap_or_default();
    vec![
        Line::plain(label),
        model_cell,
        Line::plain(effort),
        Line::styled(from, Style::fg("muted")),
        Line::plain(price(served.map(|m| m.pricing.input_per_1m_usd))),
        Line::plain(price(served.map(|m| m.pricing.output_per_1m_usd))),
    ]
}

/// Where a subagent role's model came from: picked here, the config files, or inherited.
fn from(now: &Now<'_>, kind: Option<&str>, source: Source) -> String {
    let words = &ui::get().roles;
    let own = matches!(
        (kind, source),
        (Some(_), Source::Kind) | (None, Source::Subagents)
    );
    match own {
        true if now.picked.role(kind).model.is_some() => words.picked.clone(),
        true => words.config.clone(),
        false => fill(
            &words.inherits,
            &[("from", &source.label(kind.unwrap_or_default()))],
        ),
    }
}

fn table(rows: Vec<Vec<Line>>) -> Table {
    let words = &ui::get().roles;
    let header = words
        .columns
        .iter()
        .map(|c| Line::styled(c, Style::fg("muted").bold()))
        .collect();
    let mut all = vec![header];
    all.extend(rows);
    Table {
        rows: all,
        aligns: vec![
            Align::Left,
            Align::Left,
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
        ],
        // Narrow, where a model came from goes first, then the prices.
        drop: vec![3, 5, 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;
    use crate::models::{self, Source as Origin};

    fn catalog() -> Catalog {
        Catalog {
            models: models::snapshot(),
            source: Origin::Snapshot,
            fetched_at: None,
            note: None,
        }
    }

    fn key(card: &mut Roles, action: Action) -> Next {
        card.key(Some(action), &press(KeyCode::Null))
    }

    fn text(card: &Roles) -> String {
        card.render(140)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn open(subagents: &Subagents, picked: &Subagents, role: Option<RoleId>) -> Roles {
        let catalog = catalog();
        let chat = catalog.models[0].id.clone();
        let now = Now {
            catalog: &catalog,
            chat: &chat,
            chat_effort: None,
            subagents,
            picked,
            kept_in: "prefs.json".into(),
        };
        Roles::open(&now, role)
    }

    #[test]
    fn the_list_shows_each_role_its_model_and_where_it_came_from() {
        let catalog = catalog();
        let (chat, cheap) = (&catalog.models[0].id, &catalog.models[1].id);
        let mut subagents = Subagents::default();
        subagents.set(
            Some("explore"),
            Role {
                model: Some(cheap.clone()),
                effort: None,
            },
        );
        subagents.set(
            Some("general"),
            Role {
                model: Some("gone-model".into()),
                effort: None,
            },
        );
        let mut picked = Subagents::default();
        picked.set(
            Some("explore"),
            Role {
                model: Some(cheap.clone()),
                effort: None,
            },
        );
        let shown = text(&open(&subagents, &picked, None));
        let line = |needle: &str| {
            shown
                .lines()
                .find(|l| l.contains(needle))
                .unwrap()
                .to_owned()
        };
        assert!(line("explore").contains(cheap.as_str()) && line("explore").contains("picked"));
        assert!(
            line("general").contains("gone-model (unavailable)"),
            "{shown}"
        );
        assert!(line("general").contains("config"), "{shown}");
        assert!(line("subagents").contains(chat.as_str()), "{shown}");
        assert!(
            line("subagents").contains("inherits the chat model"),
            "{shown}"
        );
    }

    #[test]
    fn a_kind_is_picked_model_then_effort_and_esc_backs_out_unchanged() {
        let catalog = catalog();
        let none = Subagents::default();
        let mut card = open(&none, &none, None);
        // chat, subagents, explore.
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        assert!(matches!(key(&mut card, Action::Submit), Next::Stay));
        assert!(text(&card).contains("explore: model"), "{}", text(&card));
        assert!(matches!(key(&mut card, Action::Escape), Next::Stay));
        assert!(text(&card).contains("subagents (every kind)"));
        key(&mut card, Action::Submit);
        // The inherit row first, then the catalog: the second model.
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        assert!(text(&card).contains("explore: effort"), "{}", text(&card));
        key(&mut card, Action::Down);
        let level = effort::levels()[0].id.clone();
        match key(&mut card, Action::Submit) {
            Next::Assign { kind, role } => {
                assert_eq!(kind.as_deref(), Some("explore"));
                assert_eq!(role.model.as_deref(), Some(catalog.models[1].id.as_str()));
                assert_eq!(role.effort.as_deref(), Some(level.as_str()));
            }
            _ => panic!("no assignment"),
        }
    }

    #[test]
    fn inherit_assigns_an_empty_role_and_the_chat_row_switches_the_session() {
        let catalog = catalog();
        let mut subagents = Subagents::default();
        subagents.set(
            Some("explore"),
            Role {
                model: Some(catalog.models[1].id.clone()),
                effort: None,
            },
        );
        let explore = RoleId::Kind("explore".into());
        let mut card = open(&subagents, &subagents, Some(explore));
        // Opened on the explore model in use; up to inherit.
        key(&mut card, Action::Up);
        key(&mut card, Action::Up);
        key(&mut card, Action::Submit);
        match key(&mut card, Action::Submit) {
            Next::Assign { kind, role } => {
                assert_eq!(kind.as_deref(), Some("explore"));
                assert_eq!(role, Role::default());
            }
            _ => panic!("no assignment"),
        }

        let mut card = open(&subagents, &subagents, Some(RoleId::Chat));
        assert!(
            matches!(key(&mut card, Action::Submit), Next::UseModel(id) if id == catalog.models[0].id)
        );
    }
}
