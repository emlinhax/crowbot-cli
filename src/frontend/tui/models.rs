//! `/models`: a tab per agent — the main agent, then each role a subagent can be started as — with
//! its model, its reasoning effort and what it is for. Everything here is set here: a role's
//! description is written to its file, and `+` adds a role, so nothing needs hand-editing. Choices
//! are remembered across sessions in ~/.crowbot/prefs.json, so a cheap reader stays cheap.
//!
//! One card with stages: the form, the model table, a line for a description, a delete question.
//! The form fills the rows it is given, so the transcript never peeks through around it.

use crate::agent::roles::{self, Role};
use crate::api::models::Model;
use crate::config::paths::Paths;
use crate::config::prefs::{self, Choice};
use crate::effort;
use crate::frontend::tui::boxed::{self, boxed};
use crate::frontend::tui::card::{Card, Next};
use crate::frontend::tui::editor::{Editor, Field as EditorField};
use crate::frontend::tui::keymap::Action;
use crate::frontend::tui::rows;
use crate::frontend::tui::ui;
use crate::io::term::KeyEvent;
use crate::models::Catalog;
use crate::models::table as models;
use crate::text::styled::{Line, Style};
use crate::text::table::{self, GAP};
use crate::text::template::fill;

/// One role as the card shows it: the file's words and the machine's choice of model and effort.
#[derive(Clone)]
struct RoleView {
    role: Role,
    choice: Choice,
    /// Whether the user turned it on: subagents are off until they do.
    enabled: bool,
}

/// What a tab shows, top to bottom.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    State,
    Model,
    Effort,
    Summary,
    /// The words added to the session's system prompt for this role: its tab's lower half.
    Prompt,
    Tools,
    Delete,
    Name,
    Create,
}

/// Whose setting a choice changes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Main,
    Role(usize),
}

enum Stage {
    Form,
    /// The model table for `target`, opened on `selected`.
    Models {
        target: Target,
        selected: usize,
    },
    /// A line for one of the form's words.
    Edit {
        edit: Edit,
        editor: Editor,
    },
    /// Whether to delete `target`.
    Confirm {
        target: Target,
    },
}

/// Which words an edit line changes.
#[derive(Clone, Copy)]
enum Edit {
    Summary(Target),
    Prompt(Target),
    Name,
    DraftSummary,
    DraftPrompt,
}

pub struct Models {
    catalog: Catalog,
    paths: Paths,
    /// The session's own model and effort: the first tab, and every role's fallback.
    session: Choice,
    roles: Vec<RoleView>,
    /// The role the `+` tab is describing; its `id` is the name typed so far.
    draft: Role,
    /// 0 is the main agent, 1..=roles.len() a role, and the last tab is `+`.
    tab: usize,
    field: usize,
    stage: Stage,
}

impl Models {
    /// The card, opened on the main agent's tab.
    pub fn new(catalog: &Catalog, paths: &Paths, session: (&Model, Option<&str>)) -> Self {
        let mut card = Self {
            catalog: catalog.clone(),
            paths: paths.clone(),
            session: Choice {
                model: Some(session.0.id.clone()),
                effort: session.1.map(str::to_owned),
            },
            roles: Vec::new(),
            draft: roles::new_role("", true),
            tab: 0,
            field: 0,
            stage: Stage::Form,
        };
        card.reload();
        card
    }

    /// The roles as the machine has them now, each with the choice remembered for it.
    fn reload(&mut self) {
        let roles = roles::load(&self.paths).unwrap_or_else(|_| roles::builtin().to_vec());
        let chosen = prefs::load(&self.paths).roles;
        self.roles = roles
            .into_iter()
            .map(|role| RoleView {
                choice: chosen.get(&role.id).cloned().unwrap_or_default(),
                enabled: prefs::enabled(&self.paths, &role.id),
                role,
            })
            .collect();
        self.tab = self.tab.min(self.roles.len() + 1);
        self.field = self.field.min(self.fields().len().saturating_sub(1));
    }

    /// The last tab, which adds a role.
    fn add_tab(&self) -> usize {
        self.roles.len() + 1
    }

    /// The tab `index` stands for.
    fn target(&self, index: usize) -> Option<Target> {
        match index {
            0 => Some(Target::Main),
            i if i <= self.roles.len() => Some(Target::Role(i - 1)),
            _ => None,
        }
    }

    fn target_role(&self, target: Target) -> Option<&RoleView> {
        match target {
            Target::Main => None,
            Target::Role(i) => self.roles.get(i),
        }
    }

    /// What a target runs on: its own setting, else the session's.
    fn model_of(&self, target: Target) -> Option<String> {
        let own = match target {
            Target::Main => self.session.model.clone(),
            Target::Role(_) => self
                .target_role(target)
                .and_then(|view| view.choice.model.clone()),
        };
        own.or_else(|| self.session.model.clone())
    }

    fn effort_of(&self, target: Target) -> Option<String> {
        let own = match target {
            Target::Main => self.session.effort.clone(),
            Target::Role(_) => self
                .target_role(target)
                .and_then(|view| view.choice.effort.clone()),
        };
        own.or_else(|| self.session.effort.clone())
    }

    /// The rows of the tab now showing.
    fn fields(&self) -> Vec<Field> {
        if self.tab == 0 {
            vec![Field::Model, Field::Effort]
        } else if self.tab == self.add_tab() {
            vec![
                Field::Name,
                Field::Summary,
                Field::Prompt,
                Field::Tools,
                Field::Create,
            ]
        } else {
            vec![
                Field::State,
                Field::Model,
                Field::Effort,
                Field::Summary,
                Field::Prompt,
                Field::Tools,
                Field::Delete,
            ]
        }
    }

    /// The role the tab is about, if it is a role's: the draft for `+`.
    fn tab_role(&self) -> Option<&Role> {
        if self.tab == self.add_tab() {
            Some(&self.draft)
        } else {
            self.target(self.tab).and_then(|target| match target {
                Target::Main => None,
                Target::Role(i) => self.roles.get(i).map(|view| &view.role),
            })
        }
    }

    fn field(&self) -> Option<Field> {
        self.fields().get(self.field).copied()
    }

    /// Moves the field selection by `step`, within the tab.
    fn step(&mut self, step: isize) {
        let len = self.fields().len();
        let at = self.field as isize + step;
        self.field = at.clamp(0, len.saturating_sub(1) as isize) as usize;
    }

    /// Switches tabs by `step`, wrapping, and starts at the tab's first field.
    fn tab_step(&mut self, step: isize) {
        let len = self.add_tab() + 1;
        self.tab = (self.tab as isize + step).rem_euclid(len as isize) as usize;
        self.field = 0;
    }

    /// Whether the tab at `index` is a role the user turned on.
    fn role_on(&self, index: usize) -> bool {
        match self.target(index) {
            Some(Target::Role(i)) => self.roles.get(i).is_some_and(|view| view.enabled),
            Some(Target::Main) => true,
            None => true,
        }
    }

    /// The tab's name, for the bar and the model table's title.
    fn name(&self, index: usize) -> String {
        match index {
            0 => ui::get().models.main.clone(),
            i if i <= self.roles.len() => self.roles[i - 1].role.id.clone(),
            _ => ui::get().models.new.clone(),
        }
    }

    /// The model table's rows: the session's model first for a role, then the catalog, with the
    /// model in use marked.
    fn model_rows(&self, target: Target) -> (Vec<Vec<Line>>, Vec<Option<String>>, usize) {
        let current = self.model_of(target);
        let own = match target {
            Target::Main => self.session.model.clone(),
            Target::Role(_) => self
                .target_role(target)
                .and_then(|view| view.choice.model.clone()),
        };
        let mut rows = models::rows(&self.catalog, own.as_deref().unwrap_or_default());
        let mut choices: Vec<Option<String>> = Vec::new();
        if target != Target::Main {
            choices.push(None);
            // A row of its own after the header, in the table's own mark column.
            let mut row = vec![Line::styled(
                if own.is_none() { models::mark() } else { " " },
                Style::fg("accent"),
            )];
            row.push(Line::plain(fill(
                &ui::get().models.session_model,
                &[("model", &self.session.model.clone().unwrap_or_default())],
            )));
            rows.insert(1, row);
        }
        choices.extend(self.catalog.models.iter().map(|m| Some(m.id.clone())));
        let selected = choices.iter().position(|c| *c == own).unwrap_or_else(|| {
            // Following the session: open on its model, which is a real row.
            choices.iter().position(|c| *c == current).unwrap_or(0)
        });
        (rows, choices, selected)
    }

    /// The effort values to cycle through: the way back to the session's, then each level.
    fn efforts(&self) -> Vec<Option<String>> {
        let mut levels = vec![None];
        levels.extend(effort::ids().into_iter().map(|id| Some(id.to_owned())));
        levels
    }

    /// The effort level in force for `target`, as a choice: `None` is the session's (a role) or
    /// the model's own default (the main agent).
    fn own_effort(&self, target: Target) -> Option<String> {
        match target {
            Target::Main => self.session.effort.clone(),
            Target::Role(_) => self
                .target_role(target)
                .and_then(|view| view.choice.effort.clone()),
        }
    }

    /// Sets `target`'s model or effort, and says what changed. The main agent's goes back to the
    /// session, which knows how to switch and remember it; a role's is remembered here.
    fn apply_model(&mut self, target: Target, model: Option<String>) -> Next {
        match target {
            Target::Main => {
                let Some(id) = model else { return Next::Stay };
                self.session.model = Some(id.clone());
                Next::UseModel(id)
            }
            Target::Role(i) => {
                let view = &mut self.roles[i];
                view.choice.model = model.clone();
                prefs::remember_role(&self.paths, &view.role.id, view.choice.clone());
                let words = &ui::get().models;
                let say = match model {
                    Some(id) => fill(
                        &words.role_model,
                        &[("role", &view.role.id), ("model", &id)],
                    ),
                    None => fill(
                        &words.role_session,
                        &[("role", &view.role.id), ("what", "model")],
                    ),
                };
                Next::Roles(say)
            }
        }
    }

    fn apply_effort(&mut self, target: Target, level: Option<String>) -> Next {
        match target {
            Target::Main => {
                self.session.effort = level.clone();
                Next::UseEffort(level)
            }
            Target::Role(i) => {
                let view = &mut self.roles[i];
                view.choice.effort = level.clone();
                prefs::remember_role(&self.paths, &view.role.id, view.choice.clone());
                let words = &ui::get().models;
                let say = match level {
                    Some(level) => fill(
                        &words.role_effort,
                        &[("role", &view.role.id), ("effort", &level)],
                    ),
                    None => fill(
                        &words.role_session,
                        &[("role", &view.role.id), ("what", "effort")],
                    ),
                };
                Next::Roles(say)
            }
        }
    }

    /// Writes a role's file, keeping its model and effort choices.
    fn save(&mut self, target: Target, role: Role) -> Next {
        let Target::Role(i) = target else {
            return Next::Stay;
        };
        match roles::save(&self.paths, &role) {
            Ok(path) => {
                self.reload();
                self.tab = i + 1;
                Next::Roles(fill(
                    &ui::get().models.saved,
                    &[("id", &role.id), ("path", &path.display().to_string())],
                ))
            }
            Err(e) => Next::Error(format!("{e:#}")),
        }
    }

    /// Turns a role on or off: subagents are a feature the user opts into, and a role that is off
    /// is not named to the model and cannot be dispatched.
    fn toggle(&mut self, target: Target) -> Next {
        let Target::Role(i) = target else {
            return Next::Stay;
        };
        let (id, on) = {
            let view = &self.roles[i];
            (view.role.id.clone(), !view.enabled)
        };
        prefs::set_enabled(&self.paths, &id, on);
        self.reload();
        let words = &ui::get().models;
        let say = if on {
            fill(&words.turned_on, &[("role", &id)])
        } else {
            fill(&words.turned_off, &[("role", &id)])
        };
        Next::Roles(say)
    }

    /// Does what Enter on the selected field means.
    fn activate(&mut self) -> Next {
        let Some(field) = self.field() else {
            return Next::Stay;
        };
        let target = self.target(self.tab).unwrap_or(Target::Main);
        match field {
            Field::State => self.toggle(target),
            Field::Model => {
                let (_, _, selected) = self.model_rows(target);
                self.stage = Stage::Models { target, selected };
                Next::Stay
            }
            Field::Effort => {
                let values = self.efforts();
                let at = values
                    .iter()
                    .position(|v| *v == self.own_effort(target))
                    .unwrap_or(0);
                let level = values[(at + 1) % values.len()].clone();
                self.apply_effort(target, level)
            }
            Field::Name => self.edit(Edit::Name, self.draft.id.clone()),
            Field::Summary if self.tab == self.add_tab() => {
                self.edit(Edit::DraftSummary, self.draft.summary.clone())
            }
            Field::Summary => match target {
                Target::Role(i) => {
                    self.edit(Edit::Summary(target), self.roles[i].role.summary.clone())
                }
                Target::Main => Next::Stay,
            },
            Field::Prompt if self.tab == self.add_tab() => {
                self.edit(Edit::DraftPrompt, self.draft.prompt.clone())
            }
            Field::Prompt => match target {
                Target::Role(i) => {
                    self.edit(Edit::Prompt(target), self.roles[i].role.prompt.clone())
                }
                Target::Main => Next::Stay,
            },
            Field::Tools if self.tab == self.add_tab() => {
                let everything = self.draft.tools.iter().any(|tool| tool == "*");
                let mut draft = roles::new_role(&self.draft.id, !everything);
                draft.summary = self.draft.summary.clone();
                draft.prompt = self.draft.prompt.clone();
                self.draft = draft;
                Next::Stay
            }
            Field::Tools => Next::Stay,
            Field::Delete => {
                self.stage = Stage::Confirm { target };
                Next::Stay
            }
            Field::Create => self.create(),
        }
    }

    /// Opens a line for one of the form's words.
    fn edit(&mut self, edit: Edit, text: String) -> Next {
        let mut editor = Editor::new();
        editor.set_text(&text);
        self.stage = Stage::Edit { edit, editor };
        Next::Stay
    }

    /// Keeps what an edit line handed back.
    fn edited(&mut self, edit: Edit, text: &str) -> Next {
        match edit {
            Edit::Summary(Target::Role(_)) | Edit::DraftSummary if text.is_empty() => {
                Next::Error(ui::get().models.summary_bad.clone())
            }
            Edit::Summary(target) => {
                let Target::Role(i) = target else {
                    return Next::Stay;
                };
                let mut role = self.roles[i].role.clone();
                role.summary = text.to_owned();
                self.save(target, role)
            }
            Edit::Prompt(target) => {
                let Target::Role(i) = target else {
                    return Next::Stay;
                };
                let mut role = self.roles[i].role.clone();
                role.prompt = text.to_owned();
                self.save(target, role)
            }
            Edit::Name => {
                self.draft.id = text.to_owned();
                Next::Stay
            }
            Edit::DraftSummary => {
                self.draft.summary = text.to_owned();
                Next::Stay
            }
            Edit::DraftPrompt => {
                self.draft.prompt = text.to_owned();
                Next::Stay
            }
        }
    }

    /// Deletes a role, asking first.
    fn delete(&mut self, target: Target) -> Next {
        let Target::Role(i) = target else {
            return Next::Stay;
        };
        let Some(view) = self.roles.get(i).cloned() else {
            return Next::Stay;
        };
        match roles::remove(&self.paths, &view.role) {
            Ok(()) => {
                self.reload();
                self.tab = self.tab.min(self.roles.len());
                Next::Roles(fill(&ui::get().models.deleted, &[("id", &view.role.id)]))
            }
            Err(e) => Next::Error(format!("{e:#}")),
        }
    }

    /// Adds the role described in the `+` tab.
    fn create(&mut self) -> Next {
        let words = &ui::get().models;
        let mut role = self.draft.clone();
        role.id = role.id.trim().to_owned();
        role.summary = role.summary.trim().to_owned();
        if !roles::valid_id(&role.id) {
            return Next::Error(words.name_bad.clone());
        }
        if role.summary.is_empty() {
            return Next::Error(words.summary_bad.clone());
        }
        if self.roles.iter().any(|view| view.role.id == role.id)
            || roles::find(roles::builtin(), &role.id).is_some()
        {
            // A built-in role the user deleted comes back with this file.
            if !prefs::hidden(&self.paths, &role.id) {
                return Next::Error(fill(&words.name_taken, &[("id", &role.id)]));
            }
        }
        prefs::set_enabled(&self.paths, &role.id, true);
        match roles::save(&self.paths, &role) {
            Ok(path) => {
                let id = role.id.clone();
                self.reload();
                self.tab = self.roles.iter().position(|v| v.role.id == id).unwrap_or(0) + 1;
                self.field = 0;
                self.draft = roles::new_role("", true);
                Next::Roles(fill(
                    &words.created,
                    &[("id", &id), ("path", &path.display().to_string())],
                ))
            }
            Err(e) => Next::Error(format!("{e:#}")),
        }
    }

    /// How tall the form is when it is not given a screen to fill.
    fn natural_height(&self) -> usize {
        let mut body = self.fields().len() + 2;
        if self.tab == self.add_tab() {
            body += 2;
        }
        body + 3
    }

    /// The tab bar: each tab's name, the current one in the accent colour, scrolled so the
    /// current tab is always whole.
    fn tabs(&self, width: usize) -> Line {
        let labels: Vec<String> = (0..=self.add_tab()).map(|i| self.name(i)).collect();
        let active = self.tab;
        let sep = ui::get().models.tab_separator.clone();
        let rendered = |i: usize| format!(" {} ", labels[i]);
        // Keep the active tab in the window, moving the window only as far as it must.
        let mut first = 0;
        loop {
            let mut used = 0;
            let mut last = first;
            while last < labels.len() {
                let add = rendered(last).chars().count()
                    + if last > first { sep.chars().count() } else { 0 };
                if used + add > width {
                    break;
                }
                used += add;
                last += 1;
            }
            if last > active || last == labels.len() {
                break;
            }
            first += 1;
        }
        let mut line = Line::default();
        let mut last = first;
        let mut used = 0;
        while last < labels.len() {
            let text = rendered(last);
            let add = text.chars().count() + if last > first { sep.chars().count() } else { 0 };
            if used + add > width {
                break;
            }
            used += add;
            if last > first {
                line.push(&sep, Style::fg("muted"));
            }
            let style = if last == active {
                Style::fg("accent").bold()
            } else if self.role_on(last) {
                Style::default()
            } else {
                Style::fg("muted")
            };
            line.push(&text, style);
            last += 1;
        }
        line.truncate(width)
    }

    /// One field's row: its label, then its value.
    fn row(&self, index: usize, field: Field, width: usize) -> Line {
        let selected = index == self.field;
        let label_style = if selected {
            Style::fg("accent").bold()
        } else {
            Style::fg("muted")
        };
        let mut line = if selected {
            Line::styled(
                format!("{} ", ui::get().marks.selected),
                Style::fg("accent").bold(),
            )
        } else {
            Line::plain(" ".repeat(rows::MARK))
        };
        let (label, value) = self.field_text(field);
        line.push(format!("{label:<12} "), label_style);
        line.extend(value);
        line.truncate(width)
    }

    /// A field's label and its value, styled as it should read.
    fn field_text(&self, field: Field) -> (&'static str, Line) {
        let words = &ui::get().models;
        let muted = || Style::fg("muted");
        match field {
            Field::State => {
                let on = self
                    .tab_role()
                    .is_some_and(|role| prefs::enabled(&self.paths, &role.id));
                let (text, style) = if on {
                    (words.on.clone(), Style::fg("accent").bold())
                } else {
                    (words.off.clone(), muted())
                };
                let mut line = Line::styled(text, style);
                line.push(format!("  {}", words.state_hint), muted());
                (&words.state, line)
            }
            Field::Model => {
                let target = self.target(self.tab).unwrap_or(Target::Main);
                let own = match target {
                    Target::Main => true,
                    Target::Role(_) => self
                        .target_role(target)
                        .is_some_and(|view| view.choice.model.is_some()),
                };
                let id = self.model_of(target).unwrap_or_default();
                let mut line = Line::plain(id);
                if !own {
                    line.push(format!(" {}", words.follow_model), muted());
                }
                (&words.model, line)
            }
            Field::Effort => {
                let target = self.target(self.tab).unwrap_or(Target::Main);
                let own = self.own_effort(target);
                let effective = self.effort_of(target);
                let mut line = match &effective {
                    Some(level) => Line::styled(
                        level.clone(),
                        effort::get(level).map_or_else(Style::default, |l| Style::fg(&l.color)),
                    ),
                    None => Line::styled(words.effort_default.clone(), muted()),
                };
                if let Some(level) = effective.as_deref().and_then(effort::get) {
                    line.push(format!("  {}", level.summary), muted());
                }
                if own.is_none() && target != Target::Main {
                    // A role that follows the session says so instead of claiming the level.
                    let shown = effective
                        .clone()
                        .unwrap_or_else(|| words.effort_default.clone());
                    let mut line =
                        Line::styled(fill(&words.follow_effort, &[("effort", &shown)]), muted());
                    line.push(format!("  {}", words.follow_hint), muted());
                    return (&words.effort, line);
                }
                (&words.effort, line)
            }
            Field::Summary => {
                let target = self.target(self.tab).unwrap_or(Target::Main);
                let text = match target {
                    Target::Main => String::new(),
                    Target::Role(_) => self
                        .target_role(target)
                        .map(|view| view.role.summary.clone())
                        .unwrap_or_default(),
                };
                if text.is_empty() {
                    return (
                        &words.summary,
                        Line::styled(words.summary_hint.clone(), muted()),
                    );
                }
                (&words.summary, Line::plain(text))
            }
            Field::Tools => {
                let target = self.target(self.tab).unwrap_or(Target::Main);
                let text = match target {
                    Target::Main => String::new(),
                    Target::Role(_) => self
                        .target_role(target)
                        .map(|view| tools_text(&view.role, words))
                        .unwrap_or_default(),
                };
                let text = if text.is_empty() {
                    self.draft_tools()
                } else {
                    text
                };
                (&words.tools, Line::plain(text))
            }
            Field::Delete => (
                &words.delete,
                Line::styled(words.delete_hint.clone(), Style::fg("warn")),
            ),
            Field::Prompt => {
                let text = self
                    .tab_role()
                    .map(|role| role.prompt.trim().to_owned())
                    .unwrap_or_default();
                if text.is_empty() {
                    return (
                        &words.prompt,
                        Line::styled(words.prompt_hint.clone(), muted()),
                    );
                }
                // One row holds the first line; the pane under the form holds it all.
                let first = text.lines().next().unwrap_or_default();
                let more = text.lines().count().saturating_sub(1);
                let mut line = Line::plain(first);
                if more > 0 {
                    line.push(
                        format!("  {}", fill(&words.more_lines, &[("n", &more.to_string())])),
                        muted(),
                    );
                }
                (&words.prompt, line)
            }
            Field::Name => (
                &words.name,
                if self.draft.id.is_empty() {
                    Line::styled(words.name_hint.clone(), muted())
                } else {
                    Line::plain(self.draft.id.clone())
                },
            ),
            Field::Create => (
                &words.create,
                Line::styled(words.create_hint.clone(), Style::fg("accent")),
            ),
        }
    }

    /// The tools the draft would get, as its row says.
    fn draft_tools(&self) -> String {
        tools_text(&self.draft, &ui::get().models)
    }

    /// The form in `rows` rows: the tab bar, then the fields, scrolled so the selected one is in
    /// view, then whatever room is left for what the tab is for.
    fn form(&self, width: usize, rows: usize) -> Vec<Line> {
        let mut lines = vec![self.tabs(width), Line::default()];
        let budget = rows.saturating_sub(lines.len());
        let fields = self.fields();
        let start = (self.field + 1).saturating_sub(budget);
        for (index, field) in fields.iter().enumerate().skip(start).take(budget) {
            lines.push(self.row(index, *field, width));
        }
        let left = rows.saturating_sub(lines.len());
        if left > 0 {
            lines.push(Line::default());
            lines.extend(self.details(width, left.saturating_sub(1)));
        }
        lines
    }

    /// The room under the form, spent on what the tab actually does: the line the model is given
    /// for a role, then the words added to its prompt.
    fn details(&self, width: usize, rows: usize) -> Vec<Line> {
        let words = &ui::get().models;
        let muted = Style::fg("muted");
        let Some(role) = self.tab_role() else {
            return self.main_details(width, rows);
        };
        let on = prefs::enabled(&self.paths, &role.id);
        let note = if on {
            fill(&words.state_on_note, &[("role", &role.id)])
        } else {
            fill(&words.state_off_note, &[("role", &role.id)])
        };
        let mut lines = vec![Line::styled(note, muted.clone())];
        if on {
            lines.push(Line::styled(&words.told, muted.clone()));
            let session_model = self.session.model.clone().unwrap_or_default();
            let line = roles::line(
                role,
                &self.paths,
                &session_model,
                self.session.effort.as_deref(),
            );
            lines.extend(Line::plain(&line).wrap(width, &Line::plain("  ")));
        }
        if lines.len() >= rows {
            lines.truncate(rows);
            return lines;
        }
        lines.push(Line::default());
        lines.push(Line::styled(&words.added, muted.clone()));
        let prompt = role.prompt.trim();
        if prompt.is_empty() {
            lines.push(Line::styled(&words.prompt_hint, muted));
        } else {
            for paragraph in prompt.split('\n') {
                lines.extend(Line::plain(paragraph).wrap(width, &Line::default()));
            }
        }
        lines.truncate(rows);
        lines
    }

    /// The main agent's tab has no role to describe, so the room says what its prompt is made of.
    fn main_details(&self, width: usize, rows: usize) -> Vec<Line> {
        let words = &ui::get().models;
        let muted = Style::fg("muted");
        let on: Vec<&RoleView> = self.roles.iter().filter(|view| view.enabled).collect();
        let mut lines = vec![Line::styled(&words.system_prompt, muted.clone())];
        lines.extend(Line::plain(&words.system_prompt_note).wrap(width, &Line::default()));
        let left = rows.saturating_sub(lines.len() + 2);
        if left > 1 {
            lines.push(Line::default());
            if on.is_empty() {
                // No role is on: `task` still works, on the session's own model and tools.
                lines.extend(Line::plain(&words.fallback_note).wrap(width, &Line::default()));
            } else {
                lines.push(Line::styled(&words.told, muted.clone()));
                let session_model = self.session.model.clone().unwrap_or_default();
                for view in on {
                    let line = roles::line(
                        &view.role,
                        &self.paths,
                        &session_model,
                        self.session.effort.as_deref(),
                    );
                    lines.extend(Line::plain(&line).wrap(width, &Line::plain("  ")));
                    if lines.len() >= rows {
                        break;
                    }
                }
            }
        }
        lines.truncate(rows);
        lines
    }

    /// The delete question, and what it means for this role's file.
    fn confirm(&self, target: Target, width: usize) -> Vec<Line> {
        let words = &ui::get().models;
        let Some(view) = self.target_role(target) else {
            return Vec::new();
        };
        let built_in = roles::find(roles::builtin(), &view.role.id).is_some();
        let what = if built_in {
            words.delete_built_in.clone()
        } else {
            fill(
                &words.delete_file,
                &[(
                    "path",
                    &self
                        .paths
                        .roles_dir()
                        .join(format!("{}.toml", view.role.id))
                        .display()
                        .to_string(),
                )],
            )
        };
        let mut lines = vec![
            Line::styled(
                fill(&words.delete_title, &[("role", &view.role.id)]),
                Style::fg("warn").bold(),
            ),
            Line::default(),
        ];
        lines.extend(Line::plain(&what).wrap(width, &Line::default()));
        lines
    }
}

/// What a role's tools read as: everything but a few, or the names it has.
fn tools_text(role: &Role, words: &crate::frontend::tui::ui::ModelsText) -> String {
    if role.tools.iter().any(|tool| tool == "*") {
        if role.except.is_empty() {
            words.tools_all.clone()
        } else {
            fill(&words.tools_all_but, &[("names", &role.except.join(", "))])
        }
    } else {
        role.tools.join(", ")
    }
}

impl Card for Models {
    fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Next {
        match &mut self.stage {
            Stage::Form => match action {
                Some(step @ (Action::Up | Action::Down)) => {
                    let step = if step == Action::Up { -1 } else { 1 };
                    self.step(step);
                }
                Some(Action::Tab) => self.tab_step(1),
                Some(Action::Left | Action::Right) => {
                    let forward = action == Some(Action::Right);
                    match self.field() {
                        // The row says Enter or an arrow switches it, so both do.
                        Some(Field::State) => {
                            let target = self.target(self.tab).unwrap_or(Target::Main);
                            return self.toggle(target);
                        }
                        Some(Field::Effort) => {
                            let values = self.efforts();
                            let target = self.target(self.tab).unwrap_or(Target::Main);
                            let at = values
                                .iter()
                                .position(|v| *v == self.own_effort(target))
                                .unwrap_or(0);
                            let step = if forward { 1 } else { -1 };
                            let next = (at as isize + step).rem_euclid(values.len() as isize);
                            let level = values[next as usize].clone();
                            return self.apply_effort(target, level);
                        }
                        Some(Field::Tools) if self.tab == self.add_tab() => {
                            let everything = self.draft.tools.iter().any(|tool| tool == "*");
                            let mut draft = roles::new_role(&self.draft.id, !everything);
                            draft.summary = self.draft.summary.clone();
                            draft.prompt = self.draft.prompt.clone();
                            self.draft = draft;
                        }
                        _ => {}
                    }
                }
                Some(Action::Submit) => return self.activate(),
                Some(Action::Escape) => return Next::Close(None),
                _ => {}
            },
            Stage::Models { target, selected } => {
                let (target, at) = (*target, *selected);
                let (_, choices, _) = self.model_rows(target);
                match action {
                    Some(step @ (Action::Up | Action::Down)) => {
                        let step = if step == Action::Up { -1 } else { 1 };
                        let next = (at as isize + step)
                            .clamp(0, choices.len().saturating_sub(1) as isize)
                            as usize;
                        if let Stage::Models { selected, .. } = &mut self.stage {
                            *selected = next;
                        }
                    }
                    Some(Action::Right | Action::Submit) => {
                        let choice = choices.get(at).cloned().unwrap_or(None);
                        self.stage = Stage::Form;
                        return self.apply_model(target, choice);
                    }
                    Some(Action::Left | Action::Escape) => self.stage = Stage::Form,
                    _ => {}
                }
            }
            Stage::Edit { edit, editor } => match editor.field_key(action, key) {
                EditorField::Submit(text) => {
                    let edit = *edit;
                    self.stage = Stage::Form;
                    return self.edited(edit, text.trim());
                }
                EditorField::Cancel => self.stage = Stage::Form,
                EditorField::Edited => {}
            },
            Stage::Confirm { target } => {
                let target = *target;
                match action {
                    Some(Action::Submit) => {
                        self.stage = Stage::Form;
                        return self.delete(target);
                    }
                    Some(Action::Escape | Action::Left) => self.stage = Stage::Form,
                    _ => {}
                }
            }
        }
        Next::Stay
    }

    fn insert(&mut self, text: &str) {
        if let Stage::Edit { editor, .. } = &mut self.stage {
            editor.insert(text);
        }
    }

    fn render(&self, width: usize) -> Vec<Line> {
        self.render_in(width, self.natural_height())
    }

    fn render_in(&self, width: usize, height: usize) -> Vec<Line> {
        let words = &ui::get().models;
        let inner = boxed::inner(width);
        // Two rules and the hint under them take three rows; a card that cannot hold those is
        // not drawn at all, rather than cut off at the top by the frame.
        let Some(body_rows) = height.checked_sub(3).filter(|rows| *rows >= 1) else {
            return Vec::new();
        };
        let (title, hint, mut body) = match &self.stage {
            Stage::Form => (
                words.title.clone(),
                words.hint.clone(),
                self.form(inner, body_rows),
            ),
            Stage::Models { target, selected } => {
                let (rows, _, _) = self.model_rows(*target);
                let title = fill(&words.title_model, &[("target", &self.name(self.tab))]);
                let mut lines = vec![
                    Line::styled(&title, Style::default().bold()),
                    Line::default(),
                ];
                // The title, the blank and the table's header take three rows of the body.
                lines.extend(table_rows(
                    rows,
                    *selected,
                    body_rows.saturating_sub(3),
                    inner.saturating_sub(rows::MARK),
                ));
                (words.title.clone(), words.targets_hint.clone(), lines)
            }
            Stage::Edit { edit, editor } => {
                let title = match edit {
                    Edit::Summary(_) | Edit::DraftSummary => words.edit_summary.clone(),
                    Edit::Prompt(_) | Edit::DraftPrompt => words.edit_prompt.clone(),
                    Edit::Name => words.edit_name.clone(),
                };
                let mut lines = vec![
                    Line::styled(&title, Style::default().bold()),
                    Line::default(),
                ];
                lines.extend(editor.render(
                    inner.saturating_sub(2),
                    body_rows.saturating_sub(2).max(1),
                    &Line::plain(&words.field_prompt),
                    "",
                ));
                (words.title.clone(), words.input_hint.clone(), lines)
            }
            Stage::Confirm { target } => (
                words.title.clone(),
                words.confirm_hint.clone(),
                self.confirm(*target, inner),
            ),
        };
        body.truncate(body_rows);
        while body.len() < body_rows {
            body.push(Line::default());
        }
        let mut out = boxed(Line::styled(title, Style::default().bold()), &body, width);
        out.push(boxed::hint(&hint, width));
        out
    }
}

/// The model table's rows laid out in `width`, windowed to `max` around `selected`.
fn table_rows(rows: Vec<Vec<Line>>, selected: usize, max: usize, width: usize) -> Vec<Line> {
    let aligns = models::aligns();
    let drop = models::drop_order();
    let lines = table::plain(&rows, &aligns, &drop, width, GAP);
    let header = lines.first().cloned();
    let mut body: Vec<Line> = lines.into_iter().skip(1).collect();
    let max = max.max(1);
    let start = (selected + 1)
        .saturating_sub(max)
        .min(body.len().saturating_sub(max));
    let mut out = Vec::new();
    out.extend(header);
    out.extend(body.drain(start..).take(max).enumerate().map(|(i, line)| {
        let index = start + i;
        if index == selected {
            let mut row = Line::styled(
                format!("{} ", ui::get().marks.selected),
                Style::fg("accent").bold(),
            );
            row.extend(line);
            row
        } else {
            let mut row = Line::plain(" ".repeat(rows::MARK));
            row.extend(line);
            row
        }
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::keymap::press;
    use crate::io::term::KeyCode;
    use crate::models::Source;

    /// A home with the built-in roles on, as a user who wants subagents has them.
    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        crate::io::fs::make_dir(&paths.home).unwrap();
        for role in roles::builtin() {
            prefs::set_enabled(&paths, &role.id, true);
        }
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
        card.render_in(100, 24)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_selected_model_keeps_its_mark_in_a_window_smaller_than_the_list() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Submit);
        // To the last model, which is off the end of a short window.
        for _ in 0..20 {
            key(&mut card, Action::Down);
        }
        let Stage::Models { selected, .. } = card.stage else {
            panic!("the table stays open");
        };
        let last = catalog().models.len() - 1;
        assert_eq!(selected, last);
        let lines = card.render_in(60, 12);
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        let body = text.join("\n");
        assert!(body.contains("gemini-3.8-flash"), "{body}");
        // The arrow is on the selected model, and the table was scrolled to it.
        assert!(
            text.iter()
                .any(|l| l.contains("›") && l.contains("gemini-3.8-flash")),
            "{body}"
        );
        assert!(lines.len() <= 12, "{} lines", lines.len());
    }

    #[test]
    fn the_fields_scroll_to_keep_the_selected_one_in_view() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Tab);
        // Down to `delete`, the last field, in a card only just tall enough.
        for _ in 0..8 {
            key(&mut card, Action::Down);
        }
        let body = card
            .render_in(60, 12)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(body.contains("Delete this role"), "{body}");
        assert!(body.contains("› delete"), "{body}");
    }

    #[test]
    fn the_room_under_the_form_says_what_the_model_is_told_and_adds() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Tab);
        let body = drawn(&card);
        assert!(body.contains("the model is told"), "{body}");
        assert!(body.contains("added to the system prompt"), "{body}");
        assert!(body.contains("You are a subagent"), "{body}");
        // And the main agent's tab says where its own prompt comes from.
        let main = self::card(&paths);
        let body = drawn(&main);
        assert!(body.contains("data/prompts/system.md"), "{body}");
    }

    #[test]
    fn every_role_starts_off_and_the_state_row_turns_one_on() {
        let (dir, _) = (tempfile::tempdir().unwrap(), ());
        let paths = Paths::at(dir.path().join("home"), dir.path().join("proj"));
        crate::io::fs::make_dir(&paths.home).unwrap();
        // No role is on yet, so the main tab says a dispatch uses the session's own settings.
        let catalog = catalog();
        let model = catalog.models[0].clone();
        let mut card = Models::new(&catalog, &paths, (&model, Some("high")));
        let body = drawn(&card);
        assert!(body.contains("`general`"), "{body}");
        // A role's tab says it is off, and the row turns it on.
        key(&mut card, Action::Tab);
        assert!(drawn(&card).contains("Off: the model is not told of `reader`"));
        let Next::Roles(said) = key(&mut card, Action::Submit) else {
            panic!("the state row toggles: {}", drawn(&card));
        };
        assert!(said.contains("reader"), "{said}");
        assert!(prefs::enabled(&paths, "reader"));
        assert!(drawn(&card).contains("is on"), "{}", drawn(&card));
        // And off again.
        assert!(matches!(key(&mut card, Action::Submit), Next::Roles(_)));
        assert!(!prefs::enabled(&paths, "reader"));
    }

    #[test]
    fn a_tab_per_agent_and_every_row_fits_the_screen() {
        let (_dir, paths) = paths();
        let card = card(&paths);
        let body = drawn(&card);
        for name in ["Main", "reader", "worker", "researcher", "+"] {
            assert!(body.contains(name), "{body}");
        }
        assert!(body.contains("model"), "{body}");
        assert!(body.contains("effort"), "{body}");
        for (width, height) in [(40, 12), (60, 20), (100, 30), (140, 40)] {
            let lines = card.render_in(width, height);
            assert_eq!(lines.len(), height, "{width}x{height}");
            for line in &lines {
                assert!(line.width() <= width, "width {width}: {}", line.text());
            }
        }
    }

    #[test]
    fn the_tab_bar_keeps_the_current_tab_in_view_when_it_cannot_fit() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        for _ in 0..card.add_tab() {
            key(&mut card, Action::Tab);
        }
        // The add tab is last, so the bar has slid to show it.
        let bar = card.render_in(30, 16)[1].text();
        assert!(bar.contains('+'), "{bar}");
    }

    #[test]
    fn a_roles_model_is_chosen_in_the_table_and_remembered() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Tab);
        assert!(drawn(&card).contains("reader"), "{}", drawn(&card));
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        assert!(drawn(&card).contains("reader"), "{}", drawn(&card));
        // The session's model is the first row; the next one is a model of its own.
        key(&mut card, Action::Down);
        assert!(matches!(key(&mut card, Action::Submit), Next::Roles(_)));
        let choice = prefs::load(&paths).roles["reader"].clone();
        assert_eq!(
            choice.model.as_deref(),
            Some(catalog().models[0].id.as_str())
        );
    }

    #[test]
    fn effort_is_cycled_in_place_without_a_second_screen() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Tab);
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        assert!(drawn(&card).contains("effort"), "{}", drawn(&card));
        // It follows the session first; one step right is the lowest level.
        assert!(matches!(key(&mut card, Action::Right), Next::Roles(_)));
        let choice = prefs::load(&paths).roles["reader"].clone();
        assert_eq!(choice.effort.as_deref(), Some(effort::ids()[0]));
    }

    #[test]
    fn the_model_table_moves_and_chooses() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Submit);
        assert!(matches!(card.stage, Stage::Models { .. }));
        key(&mut card, Action::Down);
        assert!(matches!(card.stage, Stage::Models { selected: 1, .. }));
        let Next::UseModel(id) = key(&mut card, Action::Submit) else {
            panic!("the table's row is chosen");
        };
        assert_eq!(id, catalog().models[1].id);
        assert!(matches!(card.stage, Stage::Form));
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
        // Its effort row cycles in place: from high, one step right is the highest level.
        let mut card = self::card(&paths);
        key(&mut card, Action::Down);
        assert!(matches!(
            key(&mut card, Action::Right),
            Next::UseEffort(Some(_))
        ));
    }

    #[test]
    fn a_description_is_edited_in_the_card_and_written_to_the_file() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Tab);
        for _ in 0..3 {
            key(&mut card, Action::Down);
        }
        key(&mut card, Action::Submit);
        assert!(drawn(&card).contains("What it is for"), "{}", drawn(&card));
        // The line opens on the description it has; kill it and type the new one.
        key(&mut card, Action::KillToStart);
        card.insert("Reads the CHANGELOG and nothing else.");
        let Next::Roles(said) = key(&mut card, Action::Submit) else {
            panic!(
                "a description is written to the role's file: {}",
                drawn(&card)
            );
        };
        assert!(said.contains("reader"), "{said}");
        let role = roles::load(&paths)
            .unwrap()
            .into_iter()
            .find(|r| r.id == "reader")
            .unwrap();
        assert_eq!(role.summary, "Reads the CHANGELOG and nothing else.");
    }

    #[test]
    fn a_role_is_added_from_its_tab_with_a_description() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        for _ in 0..card.add_tab() {
            key(&mut card, Action::Tab);
        }
        // Name, then the description under it.
        key(&mut card, Action::Submit);
        card.insert("tester");
        key(&mut card, Action::Submit);
        key(&mut card, Action::Down);
        key(&mut card, Action::Submit);
        card.insert("Runs the tests and reports what failed.");
        key(&mut card, Action::Submit);
        // Down past the prompt and the tools to the row that creates it.
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        key(&mut card, Action::Down);
        let Next::Roles(said) = key(&mut card, Action::Submit) else {
            panic!("a role is created: {}", drawn(&card));
        };
        assert!(said.contains("tester"), "{said}");
        let role = roles::load(&paths)
            .unwrap()
            .into_iter()
            .find(|r| r.id == "tester")
            .unwrap();
        assert_eq!(role.summary, "Runs the tests and reports what failed.");
        assert!(drawn(&card).contains("tester"), "{}", drawn(&card));
    }

    #[test]
    fn deleting_a_role_asks_first_and_removes_its_file() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        // The reader's own file, so the deletion removes it.
        let mut role = roles::new_role("reader", true);
        role.summary = "Reads.".into();
        roles::save(&paths, &role).unwrap();
        card.reload();
        key(&mut card, Action::Tab);
        for _ in 0..6 {
            key(&mut card, Action::Down);
        }
        key(&mut card, Action::Submit);
        let asked = drawn(&card);
        assert!(asked.contains("Delete"), "{asked}");
        // Esc backs out; the role is still there.
        key(&mut card, Action::Escape);
        assert!(
            roles::load(&paths)
                .unwrap()
                .iter()
                .any(|r| r.id == "reader")
        );
        // Enter deletes.
        key(&mut card, Action::Submit);
        assert!(matches!(key(&mut card, Action::Submit), Next::Roles(_)));
        assert!(!crate::io::fs::exists(
            &paths.roles_dir().join("reader.toml")
        ));
        assert!(
            !roles::load(&paths)
                .unwrap()
                .iter()
                .any(|r| r.id == "reader")
        );
    }

    #[test]
    fn a_built_in_role_is_hidden_rather_than_lost() {
        let (_dir, paths) = paths();
        let mut card = card(&paths);
        key(&mut card, Action::Tab);
        for _ in 0..6 {
            key(&mut card, Action::Down);
        }
        key(&mut card, Action::Submit);
        assert!(matches!(key(&mut card, Action::Submit), Next::Roles(_)));
        assert!(
            !roles::load(&paths)
                .unwrap()
                .iter()
                .any(|r| r.id == "reader")
        );
        assert!(prefs::hidden(&paths, "reader"));
    }
}
