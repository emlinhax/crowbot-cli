//! What a session runs on: a model, or a preset that picks one for each request. Both are chosen
//! by one name, wherever a model's id goes (`--model`, config, prefs.json, a session's record).

use anyhow::anyhow;

use crate::api::models::Model;
use crate::models::Catalog;
use crate::models::preset::{Preset, Routes};

#[derive(Clone, Debug)]
pub enum Choice {
    Model(Model),
    Preset(Routes),
}

impl Choice {
    /// What it is chosen by: the model's id, or the preset's name.
    pub fn id(&self) -> &str {
        match self {
            Self::Model(model) => &model.id,
            Self::Preset(routes) => &routes.id,
        }
    }

    /// The model a request goes to when nothing else is known: the model, or what the preset's
    /// last rule takes.
    pub fn model(&self) -> &Model {
        match self {
            Self::Model(model) => model,
            Self::Preset(routes) => routes.fallback(),
        }
    }

    pub fn preset(&self) -> Option<&Routes> {
        match self {
            Self::Model(_) => None,
            Self::Preset(routes) => Some(routes),
        }
    }
}

/// The model or preset `id` names; a model wins a shared name.
pub fn find(catalog: &Catalog, presets: &[Preset], id: &str) -> anyhow::Result<Choice> {
    if let Some(model) = catalog.get(id) {
        return Ok(Choice::Model(model.clone()));
    }
    match presets.iter().find(|p| p.id == id) {
        Some(preset) => preset.resolve(catalog).map(Choice::Preset),
        // crowbot silently swaps an unknown model id for its default, so refuse it here instead.
        None => Err(anyhow!("unknown model `{id}`; see `crowbot models`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::preset;
    use crate::models::{self as catalogs, Source};

    #[test]
    fn a_name_finds_a_model_else_a_preset_else_nothing() {
        let catalog = Catalog {
            models: catalogs::snapshot(),
            source: Source::Snapshot,
            fetched_at: None,
            note: None,
        };
        let model = catalog.models[0].id.clone();
        let presets = [preset::parsed(
            "cheap",
            &format!("[[rule]]\nmodel = \"{model}\""),
        )];
        let found = find(&catalog, &presets, &model).unwrap();
        assert!(found.preset().is_none());
        assert_eq!(found.id(), model);
        let found = find(&catalog, &presets, "cheap").unwrap();
        assert_eq!(found.id(), "cheap");
        assert_eq!(found.model().id, model);
        assert!(found.preset().is_some());
        let err = find(&catalog, &presets, "nope").unwrap_err();
        assert!(err.to_string().contains("unknown model `nope`"), "{err}");
    }
}
