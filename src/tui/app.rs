//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::anyhow;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use futures_util::{FutureExt, StreamExt};
use tokio::sync::mpsc;

use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::Part;
use crate::agent::run::{self, RunCtx};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::models::{self, Catalog, Model};
use crate::app::App;
use crate::commands::{self, Effect, Outcome as CommandOutcome, Scope, Session};
use crate::io::term::{self, Button, Input, KeyCode};
use crate::io::{clipboard, clock};
use crate::limits;
use crate::mode;
use crate::session::store::Store;
use crate::session::transcript::Transcript;
use crate::text::styled::{Line, Style, TAB};
use crate::text::template;
use crate::text::theme;
use crate::text::units;
use crate::tools::{self, Registry};
use crate::tui::card::{self, Card};
use crate::tui::command::{self, Work};
use crate::tui::editor::Editor;
use crate::tui::feed::{Block, Feed};
use crate::tui::forums::{self, Forums};
use crate::tui::keymap::{self, Action};
use crate::tui::login::{self, Login};
use crate::tui::palette::{self, Palette};
use crate::tui::paste::Burst;
use crate::tui::picker::{self, Picker};
use crate::tui::queue::{self, Kind};
use crate::tui::screen::Screen;
use crate::tui::status::{self, Progress};
use crate::tui::toast::Toast;
use crate::tui::view::View;
use crate::tui::{frame, layout, ui, welcome};
use crate::{settings, update};

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, Outcome)> + 'a>>;

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
    Job(BoxFuture<'a, Done>),
    /// Put the conversation away and start a fresh one.
    NewConversation,
    /// Run beside input: a session command, or what one of them asked for.
    Work(BoxFuture<'a, Work>),
}

/// What a card's network work came to, for the card that started it.
enum Done {
    Login(login::Done),
    Forums(forums::Done),
}

pub async fn run(app: &App, initial: Option<String>) -> anyhow::Result<ExitCode> {
    let catalog = models::load(app, false).await;
    // crowbot silently swaps an unknown model id for its default, so refuse it here instead.
    let model = catalog.get(&app.settings.model).cloned().ok_or_else(|| {
        anyhow!(
            "unknown model `{}`; see `crowbot models`",
            app.settings.model
        )
    })?;
    let mode = mode::get(&app.settings.mode).ok_or_else(|| anyhow!("mode was validated"))?;
    let registry = Registry::builtin(app);
    let store = Store::create(&app.paths);
    let session_file = store.path.display().to_string();
    let transcript = Transcript::new(Some(store));
    let shared = Shared::new(mode);
    let (tx, rx) = mpsc::unbounded_channel();
    let emit = move |event: AgentEvent| {
        let _ = tx.send(event);
    };
    let turns = Turns {
        tools: &registry,
        emit: &emit,
    };

    let raw = term::Raw::fullscreen()?;
    let mut tui = Tui::new(app, catalog, model, &shared, initial, session_file);
    // An update an earlier run installed is news once; the next look is due once a day, beside
    // the session, and only records what it did.
    if let Ok(exe) = crate::io::proc::current_exe() {
        crate::io::fs::sweep_replaced(&exe);
    }
    if let Some(change) = update::take_news(&app.paths) {
        tui.feed.notice(&change.say(), "done");
    }
    let enabled = app.settings.auto_update && settings::env("CROWBOT_NO_UPDATE").is_none();
    if update::due(&update::load(&app.paths), clock::now(), enabled) {
        let paths = app.paths.clone();
        tokio::spawn(async move {
            let _ = update::now(&paths).await;
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
    /// The model for the next turn, and the system prompt written for it.
    model: Model,
    /// The effort asked for: the setting until `/effort` changes it.
    effort: Option<String>,
    system: String,
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
    /// Prompts waiting on the user; the first is on screen in place of the editor.
    cards: VecDeque<Box<dyn Card>>,
    /// The `/login` card, on screen in place of the editor while open.
    login: Option<Login>,
    /// The `/models` card, likewise.
    picker: Option<Picker>,
    /// The `/forums` card, likewise.
    forums: Option<Forums>,
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
        let mut editor = Editor::new(limits.history_max.value);
        if let Some(text) = initial {
            editor.set_text(&text);
        }
        let mut tui = Self {
            app,
            catalog,
            system: system_prompt::build(&app.paths, &model),
            model,
            effort: app.settings.effort.clone(),
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
            login: None,
            picker: None,
            forums: None,
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
            .map(|tokens| tokens * 100 / self.model.context_window.max(1))
    }

    /// What this session reports about itself to a command.
    fn session(&self) -> Session {
        Session {
            model: self.model.id.clone(),
            effort: self.model.effort(self.effort.as_deref()).map(str::to_owned),
            mode: self.shared.mode().label.clone(),
            context_pct: self.context_pct(),
            cost_micros: self.cost_micros,
            file: self.session_file.clone(),
        }
    }

    fn welcome(&self) -> welcome::Info {
        let mode = self.shared.mode();
        welcome::Info {
            version: env!("CARGO_PKG_VERSION"),
            cwd: self.app.paths.project.display().to_string(),
            model: self.model.id.clone(),
            effort: self.model.effort(self.effort.as_deref()).map(str::to_owned),
            mode_label: mode.label.clone(),
            mode_color: mode.color.clone(),
            logged_in: self.app.api.has_key(),
            braille: term::braille(),
        }
    }

    /// A new conversation: a new session file, a clean screen, nothing the model was told.
    fn fresh(&mut self, old: Transcript) -> Transcript {
        let store = Store::create(&self.app.paths);
        let put_away = std::mem::replace(&mut self.session_file, store.path.display().to_string());
        self.shared.start_over();
        // AGENTS.md may have changed (/init writes it), and the prompt is read at a start.
        self.system = system_prompt::build(&self.app.paths, &self.model);
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
        Transcript::new(Some(store))
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
        let mut job: Option<BoxFuture<'a, Done>> = None;
        let mut work: FuturesUnordered<BoxFuture<'a, Work>> = FuturesUnordered::new();
        let mut inputs = Box::pin(term::inputs());
        let mut tick = tokio::time::interval(limits::get().tui.frame_ms.ms());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let shared = self.shared;
        loop {
            self.draw(turn.is_some());
            let step = tokio::select! {
                input = inputs.next() => {
                    let Some(input) = input else { break };
                    self.input(input, turn.is_some())
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
                    let plan_file = tools::plan_file(&app.paths, &owned.id());
                    let (model, system) = (self.model.clone(), self.system.clone());
                    let effort = self.effort.clone();
                    turn = Some(Box::pin(async move {
                        let cx = RunCtx {
                            app,
                            model: &model,
                            effort: effort.as_deref(),
                            system: &system,
                            tools,
                            plan_file,
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
            if self.login.is_none() && self.forums.is_none() {
                job = None;
            }
        }
        (self.cost_micros, transcript)
    }

    fn input(&mut self, input: Input, running: bool) -> Step<'a> {
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
            // The login card's code wherever it is clicked, else the block under the pointer.
            Input::Click {
                row,
                button: Button::Right,
            } => {
                let text = self
                    .login
                    .as_ref()
                    .and_then(Login::code)
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
            Input::Paste(_) if self.picker.is_some() => Step::Continue,
            Input::Paste(text) => {
                if let Some(card) = &mut self.login {
                    card.insert(&text);
                } else if let Some(card) = &mut self.forums {
                    card.insert(&text);
                } else if let Some(card) = self.cards.front_mut() {
                    card.insert(&text);
                } else {
                    self.editor.insert(&text);
                }
                Step::Continue
            }
            Input::Key(key) if self.login.is_some() => self.login_key(&key),
            Input::Key(key) if self.forums.is_some() => self.forums_key(&key),
            Input::Key(key) if self.picker.is_some() => {
                self.picker_key(&key);
                Step::Continue
            }
            Input::Key(key) if !self.cards.is_empty() => {
                self.card_key(&key, running);
                Step::Continue
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

    /// Keys while a prompt card is up: the card answers most, the session keeps a few.
    fn card_key(&mut self, key: &crate::io::term::KeyEvent, running: bool) {
        let action = keymap::get().action(key);
        match action {
            Some(Action::CycleMode) => return self.cycle_mode(),
            Some(Action::CtrlC) if running => return self.interrupt(),
            _ => {}
        }
        let Some(card) = self.cards.front_mut() else {
            return;
        };
        if let card::Step::Answer(reply) = card.key(action, key) {
            self.shared.answer(card.id(), reply);
            self.cards.pop_front();
        }
    }

    fn login_key(&mut self, key: &crate::io::term::KeyEvent) -> Step<'a> {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            self.cycle_mode();
            return Step::Continue;
        }
        let next = match &mut self.login {
            Some(card) => card.key(action, key),
            None => return Step::Continue,
        };
        self.login_next(next)
            .map_or(Step::Continue, |job| self.login_job(job))
    }

    fn login_job(&self, job: login::Job) -> Step<'a> {
        Step::Job(login::run(self.app, job).map(Done::Login).boxed())
    }

    fn forums_key(&mut self, key: &crate::io::term::KeyEvent) -> Step<'a> {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            self.cycle_mode();
            return Step::Continue;
        }
        match &mut self.forums {
            Some(card) => {
                let next = card.key(action, key);
                self.forums_next(next)
            }
            None => Step::Continue,
        }
    }

    fn forums_next(&mut self, next: forums::Next) -> Step<'a> {
        match next {
            forums::Next::Stay => {}
            forums::Next::Run(job) => {
                return Step::Job(forums::run(self.app, job).map(Done::Forums).boxed());
            }
            forums::Next::Note(text) => self.feed.notice(&text, "done"),
            forums::Next::Error(text) => self.feed.notice(&text, "error"),
            forums::Next::Close => self.forums = None,
        }
        Step::Continue
    }

    /// Hands a card's finished work back to it; what it asks next may be more work.
    fn job_done(&mut self, done: Done) -> Step<'a> {
        match done {
            Done::Login(done) => {
                let next = self.login.as_mut().map(|card| card.finished(done));
                next.and_then(|next| self.login_next(next))
                    .map_or(Step::Continue, |job| self.login_job(job))
            }
            Done::Forums(done) => {
                // The prompt names the forums to browse; an added or removed one changes it.
                if matches!(done, forums::Done::Changed { .. }) {
                    self.system = system_prompt::build(&self.app.paths, &self.model);
                }
                match self.forums.as_mut().map(|card| card.finished(done)) {
                    Some(next) => self.forums_next(next),
                    None => Step::Continue,
                }
            }
        }
    }

    fn picker_key(&mut self, key: &crate::io::term::KeyEvent) {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            return self.cycle_mode();
        }
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.key(action) {
            picker::Step::Stay => {}
            picker::Step::Close => self.picker = None,
            picker::Step::Pick(id) => {
                self.picker = None;
                self.switch_model(&id);
            }
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
        if self.model.effort(Some(&effort)).is_none() {
            note.push_str(&template::fill(
                &text.effort_unsent,
                &[("model", &self.model.id)],
            ));
        }
        self.effort = Some(effort);
        self.feed.notice(&note, "done");
    }

    /// Uses `id` from the next turn on, for this session only.
    fn switch_model(&mut self, id: &str) {
        let Some(model) = self.catalog.get(id).cloned() else {
            return;
        };
        self.system = system_prompt::build(&self.app.paths, &model);
        self.model = model;
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
    }

    fn login_next(&mut self, next: login::Next) -> Option<login::Job> {
        match next {
            login::Next::Stay => None,
            login::Next::Run(job) => Some(job),
            login::Next::Error(error) => {
                self.feed.notice(&error, "error");
                None
            }
            login::Next::Close(message) => {
                self.login = None;
                if let Some(message) = message {
                    self.feed.notice(&message, "done");
                }
                None
            }
        }
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
            .map(|parts| text_of(&parts))
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
                self.picker = Some(Picker::new(&self.catalog, &self.model.id));
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
                Effect::Login { number: None } => self.login = Some(Login::new()),
                Effect::Login {
                    number: Some(number),
                } => {
                    self.login = Some(Login::checking());
                    return self.login_job(login::Job::Check(number));
                }
                Effect::PickModel { refresh: true } => {
                    return Step::Work(command::refresh_catalog(self.app));
                }
                Effect::PickModel { refresh: false } => {
                    self.picker = Some(Picker::new(&self.catalog, &self.model.id));
                }
                Effect::Forums { login } => match crate::forums::store::load(&self.app.paths) {
                    Ok(list) => self.forums = Some(Forums::new(list, login.as_deref())),
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
                self.cards
                    .push_back(card::from_prompt(*id, prompt, self.draw_width()));
            }
            _ => {}
        }
        if let Some(progress) = &mut self.progress {
            progress.event(&event);
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
        self.cards.retain(|card| shared.waiting(card.id()));
        // The command popup floats over the conversation instead of growing the bar.
        let mut floating = Vec::new();
        let card = if let Some(login) = &self.login {
            Some(login.render(width))
        } else if let Some(forums) = &self.forums {
            Some(forums.render(width))
        } else if let Some(picker) = &self.picker {
            Some(picker.render(width))
        } else {
            self.cards.front().map(|card| card.render(width))
        };
        match card {
            Some(lines) => live.extend(lines),
            None => {
                live.push(frame::top(mode, width));
                if !running {
                    let found = self.palette.open(&self.editor.text());
                    if !found.is_empty() {
                        floating = self.palette.render(&found, width);
                    }
                }
                let ui = ui::get();
                let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
                let rows = (height * limits::get().tui.editor_max_rows_pct.value / 100).max(3);
                live.extend(self.editor.render(width, rows, &prompt, &ui.placeholder));
            }
        }
        live.push(frame::bottom(
            mode,
            &frame::Info {
                model: &self.model.id,
                effort: self.model.effort(self.effort.as_deref()),
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
        if let Some((secs, why)) = self.feed.retry(now) {
            return status::retrying(secs, why, width);
        }
        let color = &self.shared.mode().color;
        self.progress
            .as_ref()
            .map(|p| p.render(now, color, self.braille, width))
            .unwrap_or_default()
    }
}

fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
