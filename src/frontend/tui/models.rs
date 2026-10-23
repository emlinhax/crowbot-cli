//! `/models` in a session: the main agent's model and effort, then one row per role a subagent can
//! be started as, each with the model it runs on. Picking a row walks its model, then its effort;
//! `+ Add a role…` writes a role's file. Choices are remembered across sessions in
//! ~/.crowbot/prefs.json, so a cheap reader stays cheap.
//!
//! One card with stages rather than a card per screen: the list, a choices list, and a line for a
//! new role's name. Every stage draws in the same box; Esc goes back, and from the list it closes.

use crate::api::models::Model;
use crate::config::paths::Paths;
use crate::config::prefs::{self, Choice};
use crate::effort;
use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{Card, Next};
use crate::frontend::tui::editor::{Editor, Field};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::rows;
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::models::Catalog;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;

/// A role as this card configures it.
struct Role {
    id: String,
    /// What the user chose for it: what it runs on where that is not the session's.
    choice: Choice,
}

pub struct Models {
    catalog: Catalog,
    paths: Paths,
    /// The session's own model and effort, the first row and every role's fallback.
    session: Choice,
    roles: Vec<Role>,
    stage: Stage,
    /// The list's highlighted row: the main agent's model and effort, each role's two, then the
    /// row that adds one.
    selected: usize,
}

enum Stage {
    List,
    /// The models or effort levels for `target`.
    Choose {
        target: Target,
        kind: Kind,
        selected: usize,
    },
    /// A line for a new role's name.
    Name(Editor),
}

/// Whose setting a choice changes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Main,
    Role(usize),
}

/// What a choices screen sets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Model,
    Effort,
}

impl Models {
    /// The card, opened on the main agent's row.
    pub fn new(catalog: &Catalog, paths: &Paths, session: (&Model, Option<&str>)) -> Self {
        let mut card = Self {
            catalog: catalog.clone(),
            paths: paths.clone(),
            session: Choice {
                model: Some(session.0.id.clone()),
                effort: session.1.map(str::to_owned),
            },
            roles: Vec::new(),
            stage: Stage::List,
            selected: 0,
        };
        card.reload();
        card
    }

    /// The roles as the machine has them now, each with the choice remembered for it.
    fn reload(&mut self) {
        let roles = crate::agent::roles::load(&self.paths)
            .unwrap_or_else(|_| crate::agent::roles::builtin().to_vec());
        let chosen = prefs::load(&self.paths).roles;
        self.roles = roles
            .into_iter()
            .map(|role| Role {
                choice: chosen.get(&role.id).cloned().unwrap_or_default(),
                id: role.id,
            })
            .collect();
    }

    /// How many rows the list has: two for the main agent, two for each role, and the row that
    /// adds one.
    fn rows(&self) -> usize {
        2 * self.roles.len() + 3
    }

    /// The target and setting list row `index` stands for; `None` for the row that adds a role.
    fn row_at(&self, index: usize) -> Option<(Target, Kind)> {
        let settings = [Kind::Model, Kind::Effort];
        if index < 2 {
            return Some((Target::Main, settings[index]));
        }
        let rest = index - 2;
        let role = rest / 2;
        if role < self.roles.len() {
            return Some((Target::Role(role), settings[rest % 2]));
        }
        None
    }

    /// The list row a role's first setting sits on, so the card can open on it.
    fn first_row(&self, target: Target) -> usize {
        match target {
            Target::Main => 0,
            Target::Role(i) => 2 + 2 * i,
        }
    }

    /// What the session runs on, for a role whose row says it follows it.
    fn choice(&self, target: Target) -> &Choice {
        match target {
            Target::Main => &self.session,
            Target::Role(i) => &self.roles[i].choice,
        }
    }

    /// What `target` runs on now as its own setting: `None` when it follows the session.
    fn own(&self, target: Target, kind: Kind) -> Option<String> {
        let choice = self.choice(target);
        match kind {
            Kind::Model => choice.model.clone(),
            Kind::Effort => choice.effort.clone(),
        }
    }

    /// What `target` actually runs on: its own, else the session's.
    fn runs_on(&self, target: Target, kind: Kind) -> Option<String> {
        let own = self.own(target, kind);
        own.or_else(|| match kind {
            Kind::Model => self.session.model.clone(),
            Kind::Effort => self.session.effort.clone(),
        })
    }

    /// The choices for `target`. The first row is the way back to what it follows: a role's to the
    /// session's model or effort, the main agent's to the model's own default effort. Then the
    /// catalog, or the effort levels.
    fn choices(&self, target: Target, kind: Kind) -> Vec<Option<String>> {
        let mut rows = Vec::new();
        if target != Target::Main || kind == Kind::Effort {
            rows.push(None);
        }
        rows.extend(match kind {
            Kind::Model => self
                .catalog
                .models
                .iter()
                .map(|m| Some(m.id.clone()))
                .collect::<Vec<_>>(),
            Kind::Effort => effort::ids()
                .into_iter()
                .map(|id| Some(id.to_owned()))
                .collect(),
        });
        rows
    }

    /// What a choices row says.
    fn label(&self, target: Target, kind: Kind, choice: &Option<String>) -> String {
        let words = &ui::get().models;
        let Some(id) = choice else {
            return match (kind, target) {
                (Kind::Effort, Target::Main) => words.effort_default.clone(),
                _ => match self.session_for(kind) {
                    Some(value) => fill(&words.follow, &[("value", &value)]),
                    None if kind == Kind::Model => words.session_model.clone(),
                    None => words.session_effort.clone(),
                },
            };
        };
        id.clone()
    }

    fn session_for(&self, kind: Kind) -> Option<String> {
        match kind {
            Kind::Model => self.session.model.clone(),
            Kind::Effort => self.session.effort.clone(),
        }
    }

    /// The name of what a choices screen configures.
    fn name(&self, target: Target) -> String {
        match target {
            Target::Main => ui::get().models.main.clone(),
            Target::Role(i) => self.roles[i].id.clone(),
        }
    }

    /// Opens the choices for `target`, on what it uses now.
    fn open(&mut self, target: Target, kind: Kind) {
        let choices = self.choices(target, kind);
        let current = self.runs_on(target, kind);
        let selected = choices.iter().position(|c| *c == current).unwrap_or(0);
        self.stage = Stage::Choose {
            target,
            kind,
            selected,
        };
    }

    /// Sets `target`'s setting to `choice`, `None` meaning the session's, and says so.
    fn apply(&mut self, target: Target, kind: Kind, choice: Option<String>) -> Next {
        match target {
            Target::Role(i) => {
                let role = &mut self.roles[i];
                match kind {
                    Kind::Model => role.choice.model = choice.clone(),
                    Kind::Effort => role.choice.effort = choice.clone(),
                }
                prefs::remember_role(&self.paths, &role.id, role.choice.clone());
                let words = &ui::get().models;
                let say = match (choice, kind) {
                    (Some(value), Kind::Model) => {
                        fill(&words.role_model, &[("role", &role.id), ("model", &value)])
                    }
                    (Some(value), Kind::Effort) => fill(
                        &words.role_effort,
                        &[("role", &role.id), ("effort", &value)],
                    ),
                    (None, kind) => fill(
                        &words.role_session,
                        &[
                            ("role", &role.id),
                            (
                                "what",
                                match kind {
                                    Kind::Model => "model",
                                    Kind::Effort => "effort",
                                },
                            ),
                        ],
                    ),
                };
                Next::RoleChanged(say)
            }
            Target::Main => match kind {
                Kind::Model => Next::UseModel(choice.unwrap_or_default()),
                Kind::Effort => Next::UseEffort(choice),
            },
        }
    }

    /// Writes the new role's file, or says why it cannot.
    fn named(&mut self, name: &str) -> Next {
        let words = &ui::get().models;
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Next::Error(words.name_bad.clone());
        }
        if crate::agent::roles::find(crate::agent::roles::builtin(), name).is_some()
            || self.roles.iter().any(|role| role.id == name)
        {
            return Next::Error(fill(&words.name_taken, &[("id", name)]));
        }
        match crate::agent::roles::write_new(&self.paths, name) {
            Ok(path) => {
                self.reload();
                self.selected = self.first_row(Target::Role(self.roles.len() - 1));
                self.stage = Stage::List;
                Next::Added(fill(
                    &words.created,
                    &[("id", name), ("path", &path.display().to_string())],
                ))
            }
            Err(e) => Next::Error(format!("{e:#}")),
        }
    }

    /// The list: the main agent's model and effort, each role's two, then the row that adds a
    /// role. A value a role inherits from the session is muted.
    fn list(&self) -> Vec<Line> {
        let words = &ui::get().models;
        let mut lines = self.group(Target::Main, &words.main, &self.session);
        lines.push(Line::default());
        lines.push(Line::styled(&words.roles, Style::fg("muted").bold()));
        if self.roles.is_empty() {
            lines.push(Line::styled("  —", Style::fg("muted")));
        }
        for (i, role) in self.roles.iter().enumerate() {
            lines.extend(self.group(Target::Role(i), &role.id, &role.choice));
        }
        lines.push(Line::default());
        lines.push(self.marked(self.rows() - 1, &words.new, usize::MAX));
        lines
    }

    /// A target's two rows, its model and its effort.
    fn group(&self, target: Target, name: &str, choice: &Choice) -> Vec<Line> {
        let words = &ui::get().models;
        let model = choice.model.clone().or_else(|| self.session.model.clone());
        let effort = choice
            .effort
            .clone()
            .or_else(|| self.session.effort.clone());
        let first = self.first_row(target);
        [
            (first, &words.model, model, choice.model.is_some()),
            (
                first + 1,
                &words.effort_row,
                effort,
                choice.effort.is_some(),
            ),
        ]
        .into_iter()
        .map(|(index, what, value, own)| self.setting_row(index, name, what, &value, own, target))
        .collect()
    }

    /// One row: the target (on its first row), what is set, and the value. A value inherited from
    /// the session is muted; the main agent's are always its own.
    fn setting_row(
        &self,
        index: usize,
        name: &str,
        what: &str,
        value: &Option<String>,
        own: bool,
        target: Target,
    ) -> Line {
        let words = &ui::get().models;
        let selected = index == self.selected;
        let plain = if selected {
            Style::fg("accent").bold()
        } else {
            Style::default()
        };
        let value = value.clone().unwrap_or_else(|| words.unset.clone());
        // A role's value that follows the session is muted; its own, and the main agent's, are not.
        let style = if own || target == Target::Main || selected {
            plain.clone()
        } else {
            Style::fg("muted")
        };
        let mut line = self.mark(index);
        let label = if self.first_row(target) == index {
            format!("{name:<12} ")
        } else {
            " ".repeat(13)
        };
        line.push(label, plain);
        line.push(format!("{what:<7} "), Style::fg("muted"));
        line.push(value, style);
        line
    }

    /// The selection mark, or the spaces that stand in for it.
    fn mark(&self, index: usize) -> Line {
        if index == self.selected {
            Line::styled(
                format!("{} ", ui::get().marks.selected),
                Style::fg("accent").bold(),
            )
        } else {
            Line::plain(" ".repeat(rows::MARK))
        }
    }

    fn marked(&self, index: usize, text: &str, width: usize) -> Line {
        let style = if index == self.selected {
            Style::fg("accent").bold()
        } else {
            Style::default()
        };
        let mut line = self.mark(index);
        line.push(text, style);
        line.truncate(width)
    }

    /// The choices screen: the session's own first, then the catalog or the levels.
    fn choose(&self, target: Target, kind: Kind, selected: usize) -> Vec<Line> {
        let rows: Vec<Line> = self
            .choices(target, kind)
            .iter()
            .map(|choice| {
                let own = choice.is_some();
                let style = if own {
                    Style::default()
                } else {
                    Style::fg("muted")
                };
                Line::styled(self.label(target, kind, choice), style)
            })
            .collect();
        rows::marked(rows, selected)
    }
}

impl Card for Models {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        match &mut self.stage {
            Stage::List => match action {
                Some(step @ (Action::Up | Action::Down)) => {
                    self.selected = rows::step(self.selected, step, self.rows(), false);
                }
                Some(Action::Right | Action::Submit) => match self.row_at(self.selected) {
                    Some((target, kind)) => self.open(target, kind),
                    None => self.stage = Stage::Name(Editor::new()),
                },
                Some(Action::Left | Action::Escape) => return Next::Close(None),
                _ => {}
            },
            Stage::Choose {
                target,
                kind,
                selected,
            } => {
                let (target, kind, at) = (*target, *kind, *selected);
                let choices = self.choices(target, kind);
                match action {
                    Some(step @ (Action::Up | Action::Down)) => {
                        if let Stage::Choose { selected, .. } = &mut self.stage {
                            *selected = rows::step(at, step, choices.len(), false);
                        }
                    }
                    Some(Action::Right | Action::Submit) => {
                        let choice = choices.get(at).cloned().unwrap_or(None);
                        self.stage = Stage::List;
                        self.selected = self.first_row(target) + usize::from(kind == Kind::Effort);
                        return self.apply(target, kind, choice);
                    }
                    Some(Action::Left | Action::Escape) => self.stage = Stage::List,
                    _ => {}
                }
            }
            Stage::Name(editor) => match editor.field_key(action, key) {
                Field::Submit(text) => return self.named(text.trim()),
                Field::Cancel => self.stage = Stage::List,
                Field::Edited => {}
            },
        }
        Next::Stay
    }

    fn insert(&mut self, text: &str) {
        if let Stage::Name(editor) = &mut self.stage {
            editor.insert(text);
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        let words = &ui::get().models;
        let inner = boxed::inner(width).saturating_sub(rows::MARK);
        let (title, body, hint) = match &self.stage {
            Stage::List => (words.title.clone(), self.list(), &words.hint),
            Stage::Choose {
                target,
                kind,
                selected,
            } => (
                match kind {
                    Kind::Model => fill(&words.title_model, &[("target", &self.name(*target))]),
                    Kind::Effort => fill(&words.title_effort, &[("target", &self.name(*target))]),
                },
                self.choose(*target, *kind, *selected),
                &words.targets_hint,
            ),
            Stage::Name(editor) => {
                let mut body = vec![
                    Line::styled(&words.new_note, Style::fg("muted")),
                    Line::default(),
                ];
                body.extend(editor.render(
                    inner.saturating_sub(2),
                    1,
                    &Line::plain(&words.name),
                    "",
                ));
                (words.new_title.clone(), body, &words.input_hint)
            }
        };
        // As wide as the list needs, not the whole terminal.
        let needed = boxed::outer(body.iter().map(Line::width).max().unwrap_or(0));
        let mut out = boxed(
            Line::styled(title, Style::default().bold()),
            &body,
            needed.min(width),
        );
        out.push(boxed::hint(hint, width));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;
    use crate::models::Source;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        crate::io::fs::make_dir(&paths.home).unwrap();
        (dir, paths)
    }

    fn catalog() -> Catalog {
        Catalog {
            models: crate::models::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        }
    }

    fn card(paths: &Paths) -> Models {
        let catalog = catalog();
        let model = catalog.models[0].clone();
        Models::new(&catalog, paths, (&model, Some("high")))
    }

    fn key(card: &mut Models, action: Action) -> Next {
        card.key(Some(action), &press(KeyCode::Null))
    }

    fn drawn(card: &Models) -> String {
        card.render(100)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_list_names_the_main_agent_and_every_role() {
        let (_dir, paths) = paths();
        let card = card(&paths);
        let body = drawn(&card);
        assert!(body.contains("Main"), "{body}");
        for role in ["reader", "worker", "researcher"] {
            assert!(body.contains(role), "{body}");
        }
        assert!(body.contains("Add a role"), "{body}");
        // Each target has a model row and an effort row.
        assert!(body.contains("model"), "{body}");
        assert!(body.contains("effort"), "{body}");
    }

    #[test]
    fn every_row_fits_the_width_it_is_given() {
        let (_dir, paths) = paths();
        let card = card(&paths);
        for width in [40, 60, 100, 140] {
            for line in card.render(width) {
                assert!(line.width() <= width, "width {width}: {}", line.text());
            }
        }
    }

    #[test]
    fn a_roles_model_and_effort_are_chosen_and_remembered() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        // The reader's model row is the third: Main's model and effort, then reader's model.
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        assert!(drawn(&card).contains("reader model"), "{}", drawn(&card));
        // The session's own first, then the catalog; the next one along is the second model.
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        let choice = prefs::load(&paths).roles["reader"].clone();
        assert_eq!(
            choice.model.as_deref(),
            Some(catalog().models[1].id.as_str())
        );
        assert!(choice.effort.is_none());
        // Its effort row sits right below.
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        assert!(drawn(&card).contains("reader effort"), "{}", drawn(&card));
        // It opens on the session's effort, high; two rows up is the lowest level.
        key(&mut card, Action::Up);
        key(&mut card, Action::Up);
        key(&mut card, Action::Submit);
        let choice = prefs::load(&paths).roles["reader"].clone();
        assert_eq!(
            choice.model.as_deref(),
            Some(catalog().models[1].id.as_str())
        );
        assert_eq!(choice.effort.as_deref(), Some(effort::ids()[0]));
        assert!(!drawn(&card).contains("reader effort"), "back on the list");
    }

    #[test]
    fn the_session_rows_clear_a_roles_own_choice() {
        let (_dir, paths) = paths();
        prefs::remember_role(
            &paths,
            "reader",
            Choice {
                model: Some("crow-2".into()),
                effort: Some(effort::ids()[0].into()),
            },
        );
        let mut card = card(&paths);
        // Reader's model row, then the way back to the session's model.
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        for _ in 0..20 {
            key(&mut card, Action::Up);
        }
        assert!(drawn(&card).contains("the session's"), "{}", drawn(&card));
        assert!(matches!(
            key(&mut card, Action::Submit),
            Next::RoleChanged(_)
        ));
        assert!(prefs::load(&paths).roles["reader"].model.is_none());
        // And its effort row, whose first row is the session's effort too.
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        for _ in 0..20 {
            key(&mut card, Action::Up);
        }
        assert!(matches!(
            key(&mut card, Action::Submit),
            Next::RoleChanged(_)
        ));
        assert!(!prefs::load(&paths).roles.contains_key("reader"));
    }

    #[test]
    fn the_main_agents_model_and_effort_go_back_to_the_session() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Submit);
        key(&mut card, Action::Down);
        let Next::UseModel(id) = key(&mut card, Action::Submit) else {
            panic!("the main agent's model is the session's");
        };
        assert_eq!(id, catalog().models[1].id);
        // Its effort row sits under its model row; the first choice is the model's own default.
        let mut card = self::card(&paths);
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        for _ in 0..20 {
            key(&mut card, Action::Up);
        }
        assert!(matches!(
            key(&mut card, Action::Submit),
            Next::UseEffort(None)
        ));
    }

    #[test]
    fn a_new_role_is_written_where_the_loader_reads_it() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        for _ in 0..card.rows() - 1 {
            key(&mut card, Action::Down);
        }
        key(&mut card, Action::Submit);
        card.insert("tester");
        let Next::Added(said) = key(&mut card, Action::Submit) else {
            panic!("a role's file is written: {}", drawn(&card));
        };
        assert!(said.contains("tester"), "{said}");
        assert!(crate::io::fs::exists(
            &paths.roles_dir().join("tester.toml")
        ));
        let roles = crate::agent::roles::load(&paths).unwrap();
        assert!(roles.iter().any(|role| role.id == "tester"));
        assert!(drawn(&card).contains("tester"), "{}", drawn(&card));
    }

    #[test]
    fn a_role_that_exists_or_a_name_that_is_not_is_refused() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        card.stage = Stage::Name(Editor::new());
        card.insert("reader");
        assert!(matches!(key(&mut card, Action::Submit), Next::Error(_)));
        let mut editor = Editor::new();
        editor.insert("a b");
        card.stage = Stage::Name(editor);
        assert!(matches!(key(&mut card, Action::Submit), Next::Error(_)));
        assert!(!crate::io::fs::exists(&paths.roles_dir().join("a b.toml")));
    }
}
