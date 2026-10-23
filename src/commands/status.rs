use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::account::standing;
use super::{Command, Ctx, Outcome, Session, Spec, login};
use crate::api::account::{self, Me};
use crate::api::error::ApiError;
use crate::data;
use crate::mcp::{Standing, State};
use crate::text::template::fill;
use crate::text::units::{self, Plural};

static SPEC: LazyLock<Spec> = super::spec!("status");
static TEXT: LazyLock<Text> = data::load!("commands/status.toml", "text");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    body: String,
    balance: String,
    balance_failed: String,
    effort: String,
    context: String,
    no_context: String,
    undo_files: String,
    undo_conversation: String,
    subagent: String,
    subagent_effort: String,
    mcp_none: String,
    mcp_name: String,
    mcp_found: String,
    mcp_running: Plural,
    mcp_failed: String,
    mcp_stopped: String,
    mcp_off: String,
}

pub struct Status;

impl Command for Status {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            cx.no_args(&SPEC, args)?;
            let session = cx.session.as_ref().ok_or_else(|| cx.usage(&SPEC))?;
            let login = login::status(cx)?;
            let balance = match cx.app.api.has_key() {
                true => Some(account::me(&cx.app.api).await),
                false => None,
            };
            let project = cx.app.paths.project.display().to_string();
            Ok(render(session, &login, balance, &project).into())
        })
    }
}

/// `balance` is `None` when there is no key to ask with.
fn render(
    session: &Session,
    login: &str,
    balance: Option<Result<Me, ApiError>>,
    project: &str,
) -> String {
    let account = match balance {
        None => login.to_owned(),
        Some(Ok(me)) => fill(
            &TEXT.balance,
            &[("login", login), ("standing", &standing(&me))],
        ),
        Some(Err(e)) => fill(
            &TEXT.balance_failed,
            &[("login", login), ("error", &e.to_string())],
        ),
    };
    let model = match &session.effort {
        Some(effort) => fill(
            &TEXT.effort,
            &[("model", &session.model), ("effort", effort)],
        ),
        None => session.model.clone(),
    };
    let undo = match &session.snapshots_off {
        None => TEXT.undo_files.clone(),
        Some(why) => fill(&TEXT.undo_conversation, &[("why", why)]),
    };
    let mcp = servers(&session.mcp);
    let subagents: Vec<String> = session
        .subagents
        .iter()
        .map(|s| {
            let mut fields = vec![
                ("kind", s.kind.as_str()),
                ("model", &s.model),
                ("from", &s.from),
            ];
            match &s.effort {
                Some(effort) => {
                    fields.push(("effort", effort));
                    fill(&TEXT.subagent_effort, &fields)
                }
                None => fill(&TEXT.subagent, &fields),
            }
        })
        .collect();
    let context = match session.context_pct {
        Some(pct) => fill(&TEXT.context, &[("pct", &pct.to_string())]),
        None => TEXT.no_context.clone(),
    };
    fill(
        &TEXT.body,
        &[
            ("account", &account),
            ("model", &model),
            ("subagents", &subagents.join("; ")),
            ("mode", &session.mode),
            ("context", &context),
            ("undo", &undo),
            ("mcp", &mcp),
            ("cost", &units::usd_micros(session.cost_micros)),
            ("file", &session.file),
            ("project", project),
        ],
    )
}

/// Each MCP server, where it was found, and how it stands, on one line.
fn servers(servers: &[Standing]) -> String {
    if servers.is_empty() {
        return TEXT.mcp_none.clone();
    }
    let each: Vec<String> = servers
        .iter()
        .map(|server| {
            let name = match &server.from {
                Some(from) => fill(&TEXT.mcp_found, &[("name", &server.name), ("from", from)]),
                None => fill(&TEXT.mcp_name, &[("name", &server.name)]),
            };
            (name, &server.state)
        })
        .map(|(name, state)| match state {
            State::Running { tools } => fill(&TEXT.mcp_running.of(*tools), &[("name", &name)]),
            State::Failed(why) => fill(&TEXT.mcp_failed, &[("name", &name), ("why", why)]),
            State::Stopped => fill(&TEXT.mcp_stopped, &[("name", &name)]),
            State::Off => fill(&TEXT.mcp_off, &[("name", &name)]),
        })
        .collect();
    each.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Seat;

    fn session() -> Session {
        Session {
            model: "crow-2".into(),
            effort: Some("high".into()),
            mode: "MANUAL".into(),
            context_pct: Some(12),
            cost_micros: 42_000,
            file: "/home/u/.crowbot/sessions/s.jsonl".into(),
            snapshots_off: Some("git was not found".into()),
            mcp: vec![
                Standing {
                    name: "fake".into(),
                    from: None,
                    state: State::Running { tools: 3 },
                },
                Standing {
                    name: "gone".into(),
                    from: None,
                    state: State::Failed("gone-mcp was not found".into()),
                },
                Standing {
                    name: "ida".into(),
                    from: Some("Claude Desktop, Cursor".into()),
                    state: State::Running { tools: 1 },
                },
            ],
            subagents: vec![
                Seat {
                    kind: "explore".into(),
                    model: "kimi-k3".into(),
                    effort: Some("low".into()),
                    from: "subagents.kinds.explore".into(),
                },
                Seat {
                    kind: "general".into(),
                    model: "crow-2".into(),
                    effort: None,
                    from: "the chat model".into(),
                },
            ],
        }
    }

    #[test]
    fn it_names_the_account_model_mode_context_and_cost() {
        let me = Me {
            balance_microdollars: 12_300_000,
            can_spend: true,
            ..Me::default()
        };
        let text = render(
            &session(),
            "Logged in with device key …9999.",
            Some(Ok(me)),
            "/p",
        );
        for want in [
            "Account: Logged in with device key …9999. Balance $12.30.",
            "Model: crow-2, effort high",
            "Subagents: explore on kimi-k3, effort low (subagents.kinds.explore); general on crow-2 (the chat model)",
            "Mode: MANUAL",
            "Context: 12% of the window",
            "This session: about $0.042",
            "Undo: takes back a prompt, not its files: git was not found",
            "MCP: `fake`, 3 tools; `gone` not running (gone-mcp was not found); \
             `ida` (Claude Desktop, Cursor), 1 tool",
            "`/home/u/.crowbot/sessions/s.jsonl`",
        ] {
            assert!(text.contains(want), "{want} missing from:\n{text}");
        }
        let quiet = Session {
            effort: None,
            context_pct: None,
            ..session()
        };
        let text = render(&quiet, "Not logged in.", None, "/p");
        assert!(text.contains("Account: Not logged in.\n"), "{text}");
        assert!(
            text.contains("Model: crow-2\n") && text.contains("nothing sent yet"),
            "{text}"
        );
    }
}
