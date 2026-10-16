//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

mod account;
mod draw;
mod effects;
mod input;
mod turn;

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
use crate::api::models::Model;
use crate::app::App;
use crate::commands::Session;
use crate::config::limits;
use crate::frontend::tui::card::{self, Card};
use crate::frontend::tui::command::Work;
use crate::frontend::tui::editor::Editor;
use crate::frontend::tui::feed::Feed;
use crate::frontend::tui::palette::Palette;
use crate::frontend::tui::paste::Burst;
use crate::frontend::tui::progress::Progress;
use crate::frontend::tui::queue::Kind;
use crate::frontend::tui::screen::Screen;
use crate::frontend::tui::toast::Toast;
use crate::frontend::tui::view::View;
use crate::frontend::tui::{ui, welcome};
use crate::io::clock;
use crate::io::term;
use crate::live::event::{AgentEvent, Ended};
use crate::live::state::Shared;
use crate::models::Catalog;
use crate::release::{self, update};
use crate::session::transcript::Transcript;
use crate::text::template;
use crate::text::theme;
use crate::text::units;
use crate::tools::Registry;
use account::AccountView;
use turn::NextTurn;

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
    if let Some(change) = update::on_start(&app.http, &app.paths, app.settings.auto_update) {
        tui.feed.notice(&change.say(), "done");
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
    account: AccountView,
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
            account: AccountView::new(),
        };
        tui.feed.set_welcome(tui.welcome());
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
            warning: self.account_warning(),
            braille: self.braille,
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
        self.feed.set_welcome(self.welcome());
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
        let _shared = self.shared;
        loop {
            if let Some(run) = self.account_work() {
                work.push(run);
            }
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
                // Typed text queues while a turn runs; only a command's message gets here.
                Step::Send(text) => match transcript.take() {
                    Some(owned) => turn = Some(self.send(text, owned, &turns)),
                    None => self.feed.notice(&ui::get().text.busy, "warn"),
                },
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

    fn agent(&mut self, event: AgentEvent) {
        let now = clock::instant();
        match &event {
            AgentEvent::MessageEnd { message } => {
                if let Some(usage) = message.usage {
                    self.cost_micros += usage.cost_micros;
                    self.context_tokens = Some(usage.input);
                }
                if let Some(error) = &message.error {
                    self.account_follow(error);
                }
                self.account_reply_ended();
            }
            AgentEvent::RunEnd(_) => self.account_run_ended(),
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
}
