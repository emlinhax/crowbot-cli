//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use tokio::sync::mpsc;

use crate::agent::setup::Setup;
use crate::agent::{RunCtx, run};
use crate::api::models::Model;
use crate::app::App;
use crate::commands::{self, Effect, Outcome as CommandOutcome, Scope, Session};
use crate::config::limits;
use crate::conversation::message::Part;
use crate::frontend::tui::card::{self, Card, Next};
use crate::frontend::tui::command::{self, Work};
use crate::frontend::tui::editor::Editor;
use crate::frontend::tui::feed::{Block, Feed};
use crate::frontend::tui::forums::{self, Forums};
use crate::frontend::tui::keymap::{self, Action};
use crate::frontend::tui::login::{self, Login};
use crate::frontend::tui::next_turn::NextTurn;
use crate::frontend::tui::palette::{self, Palette};
use crate::frontend::tui::paste::Burst;
use crate::frontend::tui::picker::Picker;
use crate::frontend::tui::progress::Progress;
use crate::frontend::tui::queue::{self, Kind};
use crate::frontend::tui::screen::Screen;
use crate::frontend::tui::toast::Toast;
use crate::frontend::tui::view::View;
use crate::frontend::tui::{bar, layout, ui, welcome};
use crate::io::term::{self, Button, Input, KeyCode, KeyEvent};
use crate::io::{clipboard, clock};
use crate::live::event::{AgentEvent, Ended};
use crate::live::state::Shared;
use crate::mode;
use crate::models::Catalog;
use crate::release::{self, update};
use crate::session::transcript::Transcript;
use crate::text::styled::{Line, Style, TAB};
use crate::text::template;
use crate::text::theme;
use crate::text::units;
use crate::tools::Registry;

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, Ended)> + 'a>>;

/// What every turn shares. The model and system prompt are not here: the session can switch
/// them between turns.
struct Turns<'a> {
    tools: &'a Registry,
    emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}

/// What the loop should do after an input.
enum Step<'a> {
    Continue,
    Send(String),
    Quit,
    /// Start network work for the card on screen.
    Job(BoxFuture<'a, card::Done>),
    /// Put the conversation away and start a fresh one.
    NewConversation,
    /// Run beside input: a session command, or what one of them asked for.
    Work(BoxFuture<'a, Work>),
}

pub async fn run(app: &App, initial: Option<String>) -> anyhow::Result<ExitCode> {
    let Setup {
        catalog,
        model,
        tools,
        shared,
        transcript,
    } = Setup::load(app).await?;
    let session_file = transcript.file().display().to_string();
    let (tx, rx) = mpsc::unbounded_channel();
    let emit = move |event: AgentEvent| {
        let _ = tx.send(event);
    };
    let turns = Turns {
        tools: &tools,
        emit: &emit,
    };

    let raw = term::Raw::fullscreen()?;
    let mut tui = Tui::new(app, catalog, model, &shared, initial, session_file);
    // An update an earlier run installed is news once; the next look is due once a day, beside
    // the session, and only records what it did.
    if let Ok(exe) = crate::io::exe::current() {
        crate::io::exe::sweep(&exe);
    }
    if let Some(change) = update::take_news(&app.paths) {
        tui.feed.notice(&change.say(), "done");
    }
    if update::due(
        &update::load(&app.paths),
        clock::now(),
        app.settings.auto_update,
    ) {
        let (http, paths) = (app.http.clone(), app.paths.clone());
        tokio::spawn(async move {
            let _ = update::now(&http, &paths, update::Tell::NextStart).await;
        });
    }
    let (spent, transcript) = tui.run(turns, transcript, rx).await;
    drop(raw);

    let text = &ui::get().text;
    // Only a file that still holds the whole conversation counts as saved; a turn still
    // holding the transcript at exit has already written its prompt there.
    let last = match &transcript {
        Some(transcript) => transcript.path().map(|p| p.display().to_string()),
        None => Some(tui.session_file.clone()),
    };
    for path in tui.earlier.iter().chain(&last) {
        term::out(&format!(
            "{}\n",
            template::fill(&text.saved, &[("path", path)])
        ));
    }
    if spent > 0 {
        let cost = units::usd_micros(spent);
        term::out(&format!(
            "{}\n",
            template::fill(&text.spent, &[("cost", &cost)])
        ));
    }
    Ok(ExitCode::SUCCESS)
}

struct Tui<'a> {
    app: &'a App,
    catalog: Catalog,
    next: NextTurn,
    shared: &'a Shared,
    screen: Screen,
    feed: Feed,
    view: View,
    /// The transcript block on each screen row of the last frame, for clicks.
    blocks: Vec<Option<usize>>,
    editor: Editor,
    palette: Palette,
    burst: Burst,
    queue: Vec<(Kind, String)>,
    /// Prompts waiting on the user, each with its id; the first is on screen in place of the
    /// editor.
    cards: VecDeque<(u64, Box<dyn Card>)>,
    /// The session card (`/login`, `/forums`, `/models`), on screen over any prompt while open.
    panel: Option<Box<dyn Card>>,
    /// The turn in flight, for the working line.
    progress: Option<Progress>,
    last_verb: Option<usize>,
    rng: fastrand::Rng,
    braille: bool,
    last_ctrl_c: Option<Instant>,
    toast: Option<Toast>,
    cost_micros: u64,
    /// The current conversation's session file, and those of conversations `/new` put away.
    session_file: String,
    earlier: Vec<String>,
    /// Input tokens of the last request; the bar divides by the current model's window, so a
    /// switch of model shows its own share at once.
    context_tokens: Option<u64>,
}

impl<'a> Tui<'a> {
    fn new(
        app: &'a App,
        catalog: Catalog,
        model: Model,
        shared: &'a Shared,
        initial: Option<String>,
        session_file: String,
    ) -> Self {
        let limits = &limits::get().tui;
        let (width, height) = term::size();
        let mut editor = Editor::with_history(limits.history_max.value);
        if let Some(text) = initial {
            editor.set_text(&text);
        }
        let mut tui = Self {
            app,
            catalog,
            next: NextTurn::new(&app.paths, model, app.settings.effort.clone()),
            shared,
            screen: Screen::new(width, height, theme::get(), term::color_depth()),
            feed: Feed::new(),
            view: View::default(),
            blocks: Vec::new(),
            editor,
            palette: Palette::default(),
            burst: Burst::new(limits.paste_gap_ms.ms(), limits.paste_min_keys.value),
            queue: Vec::new(),
            cards: VecDeque::new(),
            panel: None,
            progress: None,
            last_verb: None,
            rng: fastrand::Rng::new(),
            braille: term::braille(),
            last_ctrl_c: None,
            toast: None,
            cost_micros: 0,
            session_file,
            earlier: Vec::new(),
            context_tokens: None,
        };
        let welcome = tui.welcome();
        tui.feed.push(Block::Welcome(welcome));
        tui
    }

    /// Share of the current model's window the last request used.
    fn context_pct(&self) -> Option<u64> {
        self.context_tokens
            .map(|tokens| tokens * 100 / self.next.model.context_window.max(1))
    }

    /// What this session reports about itself to a command.
    fn session(&self) -> Session {
        Session {
            model: self.next.model.id.clone(),
            effort: self.next.effort().map(str::to_owned),
            mode: self.shared.mode().label.clone(),
            context_pct: self.context_pct(),
            cost_micros: self.cost_micros,
            file: self.session_file.clone(),
        }
    }

    fn welcome(&self) -> welcome::Info {
        let mode = self.shared.mode();
        welcome::Info {
            version: release::shown(),
            cwd: self.app.paths.project.display().to_string(),
            model: self.next.model.id.clone(),
            effort: self.next.effort().map(str::to_owned),
            mode_label: mode.label.clone(),
            mode_color: mode.color.clone(),
            logged_in: self.app.api.has_key(),
            braille: term::braille(),
        }
    }

    /// A new conversation: a new session file, a clean screen, nothing the model was told.
    fn fresh(&mut self, old: Transcript) -> Transcript {
        let transcript = Transcript::create(&self.app.paths);
        let put_away = std::mem::replace(
            &mut self.session_file,
            transcript.file().display().to_string(),
        );
        self.shared.start_over();
        // AGENTS.md may have changed (/init writes it), and the prompt is read at a start.
        self.next.reload(&self.app.paths);
        self.feed = Feed::new();
        let welcome = self.welcome();
        self.feed.push(Block::Welcome(welcome));
        self.view = View::default();
        self.queue.clear();
        self.context_tokens = None;
        let text = &ui::get().text;
        match old.path() {
            Some(_) => {
                self.feed.notice(
                    &template::fill(&text.new_saved, &[("path", &put_away)]),
                    "muted",
                );
                self.earlier.push(put_away);
            }
            None => self.feed.notice(&text.new_conversation, "muted"),
        }
        transcript
    }

    /// Runs until the user quits; returns what the session spent, in micro-dollars, and the
    /// transcript unless a turn still held it.
    async fn run(
        &mut self,
        turns: Turns<'a>,
        transcript: Transcript,
        mut events: mpsc::UnboundedReceiver<AgentEvent>,
    ) -> (u64, Option<Transcript>) {
        let mut transcript = Some(transcript);
        let mut turn: Option<Turn<'a>> = None;
        let mut job: Option<BoxFuture<'a, card::Done>> = None;
        let mut work: FuturesUnordered<BoxFuture<'a, Work>> = FuturesUnordered::new();
        let mut inputs = Box::pin(term::inputs());
        let mut tick = clock::ticks(limits::get().tui.frame_ms.ms());
        let shared = self.shared;
        loop {
            self.draw(turn.is_some());
            let step = tokio::select! {
                input = inputs.next() => {
                    let Some(input) = input else { break };
                    self.input(input, turn.is_some(), job.is_some())
                }
                Some(event) = events.recv() => {
                    self.agent(event);
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    Step::Continue
                }
                (owned, _) = async { turn.as_mut().expect("guarded by the branch condition").await }, if turn.is_some() => {
                    turn = None;
                    transcript = Some(owned);
                    self.progress = None;
                    self.queue.clear();
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    Step::Continue
                }
                done = async { job.as_mut().expect("guarded by the branch condition").await }, if job.is_some() => {
                    job = None;
                    self.job_done(done)
                }
                Some(done) = work.next(), if !work.is_empty() => self.finish(done),
                // Frames tick while something moves: a turn, or a toast that has to go away.
                _ = tick.tick(), if turn.is_some() || self.toast.is_some() => Step::Continue,
            };
            match step {
                Step::Continue => {}
                Step::Quit => break,
                Step::Job(run) => job = Some(run),
                Step::Work(run) => work.push(run),
                Step::Send(text) => {
                    // Typed text queues while a turn runs; only a command's message gets here.
                    let Some(mut owned) = transcript.take() else {
                        self.feed.notice(&ui::get().text.busy, "warn");
                        continue;
                    };
                    let progress = Progress::start(clock::instant(), self.last_verb, &mut self.rng);
                    self.last_verb = Some(progress.verb());
                    self.progress = Some(progress);
                    let parts = vec![Part::Text { text }];
                    let (app, tools, emit) = (self.app, turns.tools, turns.emit);
                    let next = self.next.clone();
                    turn = Some(Box::pin(async move {
                        let cx = RunCtx {
                            app,
                            model: &next.model,
                            effort: next.asked.as_deref(),
                            system: &next.system,
                            tools,
                            emit,
                        };
                        let outcome = run::run(&cx, &mut owned, shared, parts).await;
                        (owned, outcome)
                    }));
                }
                // A turn holds the transcript while it runs.
                Step::NewConversation => match transcript.take() {
                    Some(old) => transcript = Some(self.fresh(old)),
                    None => self.feed.notice(&ui::get().text.busy, "warn"),
                },
            }
            // Closing a card drops whatever it was waiting on.
            if self.panel.is_none() {
                job = None;
            }
        }
        (self.cost_micros, transcript)
    }

    /// `running` is a turn in flight, `busy` a job the session card waits on.
    fn input(&mut self, input: Input, running: bool, busy: bool) -> Step<'a> {
        // Inside a burst of keys (a paste where the terminal sends one as keystrokes), Enter
        // and Tab are text for whatever field has the keys, never commands.
        let input = match input {
            Input::Key(key) => match (self.burst.is_paste(clock::instant()), key.code) {
                (true, KeyCode::Enter) => Input::Paste("\n".into()),
                (true, KeyCode::Tab) => Input::Paste(TAB.into()),
                _ => Input::Key(key),
            },
            other => other,
        };
        match input {
            Input::Resize(width, height) => {
                self.screen.resize(width, height);
                Step::Continue
            }
            Input::Scroll(notches) => {
                let lines = limits::get().tui.scroll_lines.value as isize;
                self.view.scroll(notches * lines);
                Step::Continue
            }
            Input::Click {
                row,
                button: Button::Left,
            } => {
                if let Some(Some(block)) = self.blocks.get(row) {
                    self.feed.toggle(*block);
                }
                Step::Continue
            }
            // What the session card offers (the login code) wherever it is clicked, else the
            // block under the pointer.
            Input::Click {
                row,
                button: Button::Right,
            } => {
                let text = self
                    .panel
                    .as_ref()
                    .and_then(|card| card.copied())
                    .map(str::to_owned)
                    .or_else(|| {
                        let block = self.blocks.get(row).copied().flatten()?;
                        self.feed.copy_text(block)
                    });
                if let Some(text) = text {
                    self.copy(&text);
                }
                Step::Continue
            }
            Input::Key(key)
                if matches!(
                    keymap::get().action(&key),
                    Some(Action::PageUp | Action::PageDown)
                ) =>
            {
                let page = self.view.page();
                let up = keymap::get().action(&key) == Some(Action::PageUp);
                self.view.scroll(if up { -page } else { page });
                Step::Continue
            }
            Input::Paste(text) => {
                if let Some(card) = &mut self.panel {
                    card.insert(&text);
                } else if let Some((_, card)) = self.cards.front_mut() {
                    card.insert(&text);
                } else {
                    self.editor.insert(&text);
                }
                Step::Continue
            }
            Input::Key(key) if self.panel.is_some() || !self.cards.is_empty() => {
                self.card_key(&key, running, busy)
            }
            Input::Key(key) => {
                match keymap::get().action(&key) {
                    Some(action) => {
                        // The command popup takes its keys first; it is closed during a run,
                        // where Tab steers.
                        if !running {
                            match self.palette.key(action, &self.editor.text()) {
                                palette::Step::Ignored => {}
                                palette::Step::Handled => return Step::Continue,
                                palette::Step::Complete(text) => {
                                    self.editor.set_text(&text);
                                    return Step::Continue;
                                }
                                palette::Step::Run(name) => {
                                    self.editor.set_text(&Scope::Session.invoke(&name));
                                    return self.submit(running);
                                }
                            }
                        }
                        self.action(action, running)
                    }
                    None => {
                        self.editor.type_key(&key);
                        Step::Continue
                    }
                }
            }
        }
    }

    /// Keys while a card is up: the session keeps a few, the card takes the rest.
    fn card_key(&mut self, key: &KeyEvent, running: bool, busy: bool) -> Step<'a> {
        let action = keymap::get().action(key);
        match action {
            Some(Action::CycleMode) => {
                self.cycle_mode();
                return Step::Continue;
            }
            Some(Action::CtrlC) if running => {
                self.interrupt();
                return Step::Continue;
            }
            // A session card waiting on its job closes, which drops the job.
            Some(Action::Escape | Action::CtrlC) if busy && self.panel.is_some() => {
                self.panel = None;
                return Step::Continue;
            }
            _ => {}
        }
        if let Some(card) = &mut self.panel {
            let next = card.key(action, key);
            return self.panel_next(next);
        }
        if let Some((id, card)) = self.cards.front_mut()
            && let Next::Answer(reply) = card.key(action, key)
        {
            self.shared.prompts.answer(*id, reply);
            self.cards.pop_front();
        }
        Step::Continue
    }

    /// Does what the session card asked for.
    fn panel_next(&mut self, next: Next) -> Step<'a> {
        match next {
            // A session card answers no prompt.
            Next::Stay | Next::Answer(_) => {}
            Next::Run(job) => return Step::Job(card::run(self.app, job)),
            Next::Note(text) => self.feed.notice(&text, "done"),
            Next::Error(text) => self.feed.notice(&text, "error"),
            Next::Close(message) => {
                self.panel = None;
                if let Some(message) = message {
                    self.feed.notice(&message, "done");
                }
            }
            Next::UseModel(id) => {
                self.panel = None;
                self.switch_model(&id);
            }
        }
        Step::Continue
    }

    /// Hands a card's finished work back to it; what it asks next may be more work.
    fn job_done(&mut self, done: card::Done) -> Step<'a> {
        // The prompt names the forums to browse; an added or removed one changes it.
        if matches!(done, card::Done::Forums(forums::Done::Changed { .. })) {
            self.next.reload(&self.app.paths);
        }
        match self.panel.as_mut().map(|card| card.finished(done)) {
            Some(next) => self.panel_next(next),
            None => Step::Continue,
        }
    }

    /// Puts `text` on the clipboard and says so at the top right.
    fn copy(&mut self, text: &str) {
        clipboard::copy(text);
        self.toast = Some(Toast::new(&ui::get().text.copied, clock::instant()));
    }

    /// Asks for `effort` from the next turn on, for this session only.
    fn set_effort(&mut self, effort: String) {
        let text = &ui::get().text;
        let mut note = template::fill(&text.effort_set, &[("effort", &effort)]);
        if self.next.model.effort(Some(&effort)).is_none() {
            note.push_str(&template::fill(
                &text.effort_unsent,
                &[("model", &self.next.model.id)],
            ));
        }
        self.next.asked = Some(effort);
        self.feed.notice(&note, "done");
    }

    /// Uses `id` from the next turn on, for this session only.
    fn switch_model(&mut self, id: &str) {
        let Some(model) = self.catalog.get(id).cloned() else {
            return;
        };
        self.next.switch(&self.app.paths, model);
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
    }

    fn action(&mut self, action: Action, running: bool) -> Step<'a> {
        match action {
            Action::Submit => return self.submit(running),
            // A command is never steering: it would reach the model as text.
            Action::Tab if running && self.is_command() => return self.submit(running),
            Action::Tab if running => self.enqueue(Kind::Steering),
            Action::Tab => self.complete(),
            Action::CycleMode => self.cycle_mode(),
            Action::Escape if running => self.interrupt(),
            Action::Escape => {}
            Action::CtrlC => {
                if !self.editor.is_empty() {
                    self.editor.clear();
                } else if running {
                    self.interrupt();
                } else {
                    let now = clock::instant();
                    let window = limits::get().tui.quit_window_ms.ms();
                    if self
                        .last_ctrl_c
                        .is_some_and(|t| now.duration_since(t) <= window)
                    {
                        return Step::Quit;
                    }
                    self.last_ctrl_c = Some(now);
                }
            }
            Action::Quit if self.editor.is_empty() => return Step::Quit,
            Action::Quit => {}
            Action::ToggleReasoning => self.feed.toggle_all(),
            editing => {
                self.editor.apply(editing);
            }
        }
        Step::Continue
    }

    fn submit(&mut self, running: bool) -> Step<'a> {
        let text = self.editor.text();
        // A trailing backslash asks for a new line, for terminals that cannot send Shift+Enter.
        if let Some(stripped) = text.strip_suffix('\\') {
            self.editor.set_text(&format!("{stripped}\n"));
            return Step::Continue;
        }
        if text.trim().is_empty() {
            return Step::Continue;
        }
        let prefix = Scope::Session.prefix();
        let escape = prefix.repeat(2);
        // A doubled prefix sends the line as written, less one `/`: `//etc/hosts is odd`.
        if text.trim_start().starts_with(&escape) {
            self.editor.set_text(&text.replacen(&escape, prefix, 1));
        } else if let Some(line) = text.trim().strip_prefix(prefix) {
            // Checked before a run gets the text: a command queued as a message would send
            // its arguments (a `--key`) to the model.
            let line = line.to_owned();
            let shown = Scope::Session.invoke(&commands::redact_line(&line));
            self.editor.take_remembering(shown);
            return self.command(&line);
        }
        if running {
            self.enqueue(Kind::Queued);
            return Step::Continue;
        }
        let text = self.editor.take();
        self.feed.user(&text);
        Step::Send(text)
    }

    /// Hands typed text to the run: steering lands after the current tools, a queued message
    /// when the run would otherwise stop.
    fn enqueue(&mut self, kind: Kind) {
        let text = self.editor.take();
        if text.trim().is_empty() {
            return;
        }
        let parts = vec![Part::Text { text: text.clone() }];
        match kind {
            Kind::Steering => self.shared.steer(parts),
            Kind::Queued => self.shared.follow_up(parts),
        }
        self.queue.push((kind, text));
    }

    /// Stops the run and puts anything still queued back in the editor.
    fn interrupt(&mut self) {
        self.shared.interrupt();
        let mut pulled: Vec<String> = self
            .shared
            .drain_steer()
            .into_iter()
            .chain(self.shared.drain_follow())
            .map(|parts| Part::text_of(&parts))
            .collect();
        self.queue.clear();
        if !pulled.is_empty() {
            if !self.editor.is_empty() {
                pulled.push(self.editor.text());
            }
            self.editor.set_text(&pulled.join("\n"));
        }
    }

    fn cycle_mode(&mut self) {
        let modes = mode::all();
        let current = self.shared.mode();
        let at = modes.iter().position(|m| m.id == current.id).unwrap_or(0);
        self.shared.set_mode(&modes[(at + 1) % modes.len()]);
    }

    /// Completes a slash command name when only one fits.
    fn complete(&mut self) {
        let text = self.editor.text();
        if let [only] = palette::matches(&text).as_slice() {
            self.editor
                .set_text(&Scope::Session.invoke(&format!("{} ", only.name)));
        }
    }

    fn is_command(&self) -> bool {
        self.editor
            .text()
            .trim_start()
            .starts_with(Scope::Session.prefix())
    }

    fn command(&mut self, line: &str) -> Step<'a> {
        match command::start(self.app, line, self.session()) {
            Ok(started) => {
                self.feed.user(&started.echo);
                Step::Work(started.run)
            }
            Err(note) => {
                if !note.is_empty() {
                    self.feed.notice(&note, "warn");
                }
                Step::Continue
            }
        }
    }

    /// Applies what a command finished with.
    fn finish(&mut self, done: Work) -> Step<'a> {
        let outcome: CommandOutcome = match done {
            Work::Catalog(catalog) => {
                self.catalog = catalog;
                self.panel = Some(Box::new(Picker::new(&self.catalog, &self.next.model.id)));
                return Step::Continue;
            }
            Work::Command(Err(e)) => {
                self.feed.notice(&format!("{e:#}"), "error");
                return Step::Continue;
            }
            Work::Command(Ok(outcome)) => outcome,
        };
        if !outcome.text.is_empty() {
            self.feed.markdown(&outcome.text);
        }
        for effect in outcome.effects {
            match effect {
                Effect::Quit => return Step::Quit,
                Effect::CycleMode => self.cycle_mode(),
                Effect::SetMode(id) => {
                    if let Some(mode) = mode::get(&id) {
                        self.shared.set_mode(mode);
                    }
                }
                Effect::Login { number: None } => self.panel = Some(Box::new(Login::new())),
                Effect::Login {
                    number: Some(number),
                } => {
                    self.panel = Some(Box::new(Login::checking()));
                    let job = card::Job::Login(login::Job::Check(number));
                    return Step::Job(card::run(self.app, job));
                }
                Effect::PickModel { refresh: true } => {
                    return Step::Work(command::refresh_catalog(self.app));
                }
                Effect::PickModel { refresh: false } => {
                    self.panel = Some(Box::new(Picker::new(&self.catalog, &self.next.model.id)));
                }
                Effect::Forums { login } => match Forums::open(&self.app.paths, login.as_deref()) {
                    Ok(card) => self.panel = Some(Box::new(card)),
                    Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
                },
                Effect::NewConversation => return Step::NewConversation,
                Effect::SetEffort(id) => self.set_effort(id),
                Effect::Send(text) => return Step::Send(text),
                Effect::CopyLastReply => match self.feed.last_reply() {
                    Some(text) => self.copy(&text),
                    None => self.feed.notice(&ui::get().text.nothing_to_copy, "muted"),
                },
            }
        }
        Step::Continue
    }

    fn agent(&mut self, event: AgentEvent) {
        let now = clock::instant();
        match &event {
            AgentEvent::MessageEnd { message } => {
                if let Some(usage) = message.usage {
                    self.cost_micros += usage.cost_micros;
                    self.context_tokens = Some(usage.input);
                }
            }
            AgentEvent::Delivered { text } => {
                if let Some(i) = self.queue.iter().position(|(_, t)| t == text) {
                    self.queue.remove(i);
                }
            }
            AgentEvent::Prompt { id, prompt, .. } => {
                let card = card::from_prompt(prompt, self.draw_width());
                self.cards.push_back((*id, card));
            }
            _ => {}
        }
        if let Some(progress) = &mut self.progress {
            progress.event(&event, now);
        }
        self.feed.event(&event, now);
    }

    fn draw(&mut self, running: bool) {
        let now = clock::instant();
        let (pad, width) = layout::margins(self.screen.width());
        let height = self.screen.height();
        let mode = self.shared.mode();
        // The bar, pinned to the bottom of the screen.
        let mut live = Vec::new();
        if running {
            live.push(self.status(now, width));
        }
        live.extend(queue::render(&self.queue, width));
        if let Some(at) = self.last_ctrl_c
            && now.duration_since(at) <= limits::get().tui.quit_window_ms.ms()
            && !running
        {
            live.push(Line::styled(
                &ui::get().text.ctrl_c_again,
                Style::fg("muted"),
            ));
        }
        // Prompts answered elsewhere (AUTO approved them, the run was interrupted) go away.
        let shared = self.shared;
        self.cards.retain(|(id, _)| shared.prompts.waiting(*id));
        // The command popup floats over the conversation instead of growing the bar.
        let mut floating = Vec::new();
        let card = match &self.panel {
            Some(card) => Some(card.render(width)),
            None => self.cards.front().map(|(_, card)| card.render(width)),
        };
        match card {
            Some(lines) => live.extend(lines),
            None => {
                live.push(bar::top(mode, width));
                if !running {
                    let found = self.palette.open(&self.editor.text());
                    if !found.is_empty() {
                        floating = self.palette.render(&found, width);
                    }
                }
                let ui = ui::get();
                let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
                let rows = (height * limits::get().tui.editor_max_rows_pct.value / 100)
                    .max(limits::get().tui.editor_min_rows.value);
                live.extend(self.editor.render(width, rows, &prompt, &ui.placeholder));
            }
        }
        live.push(bar::bottom(
            mode,
            &bar::Info {
                model: &self.next.model.id,
                effort: self.next.effort(),
                context_pct: self.context_pct(),
                cost_micros: self.cost_micros,
            },
            width,
        ));
        let mut frame = layout::compose(
            &mut self.feed,
            &mut self.view,
            &live,
            &floating,
            (width, height),
            now,
        );
        self.blocks = frame.blocks;
        if self.toast.as_ref().is_some_and(|t| !t.shown(now)) {
            self.toast = None;
        }
        if let Some(toast) = &self.toast {
            toast.overlay(&mut frame.rows, width);
        }
        layout::inset(&mut frame.rows, pad);
        let bytes = self.screen.frame(&frame.rows);
        if !bytes.is_empty() {
            term::out(&bytes);
        }
    }

    /// The columns a frame is drawn in, inside the side margins.
    fn draw_width(&self) -> usize {
        layout::margins(self.screen.width()).1
    }

    fn status(&self, now: Instant, width: usize) -> Line {
        let color = &self.shared.mode().color;
        self.progress
            .as_ref()
            .map(|p| p.render(now, color, self.braille, width))
            .unwrap_or_default()
    }
}
