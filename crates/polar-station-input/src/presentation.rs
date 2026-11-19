//! The single native projection boundary for all Station input modes.
//! Domain effects remain domain-owned; native adapters never fabricate a
//! legacy Composer to render a terminal draft.
use crate::{generic::{GenericView, Lease}, input::{Effect, GenericEffect}};
use polar_station_lgp::Effect as ContentEffect;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum NativeEffect {
    InputBar { view: InputBarView },
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputBarView {
    Generic {
        window_id: String,
        pane_id: String,
        draft_revision: u64,
        status: String,
        generic_lease: Lease,
        #[serde(flatten)]
        view: Box<GenericView>,
    },
    Chat {
        #[serde(flatten)]
        content: Box<ContentEffect>,
    },
    Idea {
        #[serde(flatten)]
        content: Box<ContentEffect>,
    },
}

/// Called by every native ABI, including its legacy aliases. Conversion stays
/// here rather than in FFI, Swift, GTK or an independent presentation engine.
pub fn native_effects(effects: Vec<Effect>) -> Result<Vec<Value>, serde_json::Error> {
    effects.into_iter().map(|effect| {
        let view = match effect {
            Effect::Generic(GenericEffect::InputBar { view }) => InputBarView::Generic {
                window_id: view.lease.window.clone(),
                pane_id: view.lease.pane.clone(),
                draft_revision: view.lease.draft_revision,
                status: view.unavailable.clone().unwrap_or_else(|| "Ready".into()),
                generic_lease: view.lease.clone(),
                view,
            },
            Effect::Content(content) if matches!(*content, ContentEffect::Composer { .. }) => InputBarView::Chat { content },
            Effect::Content(content) if matches!(*content, ContentEffect::IdeaComposer { .. }) => InputBarView::Idea { content },
            other => return serde_json::to_value(other),
        };
        serde_json::to_value(NativeEffect::InputBar { view })
    }).collect()
}
