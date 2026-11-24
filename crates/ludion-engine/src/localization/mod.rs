//! Localization: string tables, the shipped locale list, the active locale and keyed text
//! (`ludion docs localization`).
//!
//! A game opts in with [`EngineBuilder::localization`](crate::EngineBuilder::localization) (which
//! [`EngineBuilder::dialogue`](crate::EngineBuilder::dialogue) implies). `kind: strings` holds one
//! locale's key → text map, `kind: locales` names the default. The active locale is the hashed
//! [`LocaleState`] component on one lazily spawned entity, so it rides snapshots and checkpoints;
//! [`set_locale`] changes it. A [`UiText`] or
//! [`Text`] with a `key` shows the key's string through the engine's
//! `localize_text` system, which writes the runtime-only `resolved` field and **never** the authored
//! `text` — a scene saved under any locale is byte-identical.

mod docs;
mod strings;

use ludion_components::{Text, UiText};
use ludion_core::codes::code;
use ludion_core::diagnostics::Diagnostic;
use ludion_core::ids::EntityId;
use ludion_ecs::{SystemCtx, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use docs::{LocaleInfo, LocalesDoc, StringsDoc};
pub use strings::{format_text, text};

pub(crate) use docs::{
    LOCALES_KIND, STRINGS_KIND, install_content_docs, key_tables, string_in,
    validate_localization_docs,
};
pub(crate) use strings::{StringIndex, literals, render};

#[cfg(test)]
#[path = "content_tests.rs"]
mod content_tests;

/// The locale a game without a `kind: locales` doc uses.
const FALLBACK_LOCALE: &str = "en";

/// Marks an engine that opted in with `.localization()`; an unhashed resource.
#[derive(Debug, Default)]
pub(crate) struct LocalizationEnabled;

/// The active locale, one lazily spawned entity (like the dialogue control): hashed and snapshotted.
///
/// ```
/// use ludion_engine::localization::LocaleState;
///
/// let state: LocaleState = serde_json::from_value(serde_json::json!({})).unwrap();
/// assert_eq!(state.locale, None);
/// ```
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq, Default)]
pub struct LocaleState {
    /// The active locale tag; none means the default locale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}

/// The default locale: the `default` of the `kind: locales` doc, else `en`.
///
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::localization;
///
/// assert_eq!(localization::default_locale(&World::new()), "en");
/// ```
#[must_use]
pub fn default_locale(world: &World) -> String {
    world
        .resource::<crate::DocStore<LocalesDoc>>()
        .and_then(|store| store.iter().next().map(|(_, doc)| doc.default.clone()))
        .filter(|default| !default.trim().is_empty())
        .unwrap_or_else(|| FALLBACK_LOCALE.to_owned())
}

fn state_entity(world: &mut World) -> EntityId {
    if let Some(id) = world.query_ids::<(&LocaleState,)>().map(|(id, _)| id).min() {
        return id;
    }
    let id = world.spawn();
    let _ = world.insert(id, LocaleState::default());
    id
}

/// Sets the active locale; keyed text and live subtitles re-resolve on the next tick. A locale with
/// no `kind: strings` table still applies (every key falls back to the default locale) and warns
/// `L5140` once per tag. A no-op without `.localization()`.
///
/// # Example
/// ```
/// use ludion_engine::{Engine, localization};
///
/// let mut engine = Engine::builder().localization().build().unwrap();
/// assert_eq!(localization::locale(engine.world_mut()), "en");
/// localization::set_locale(engine.world_mut(), "fr");
/// assert_eq!(localization::locale(engine.world_mut()), "fr");
/// ```
pub fn set_locale(world: &mut World, locale: &str) {
    if world.resource::<LocalizationEnabled>().is_none() {
        return;
    }
    let known = key_tables(world).contains_key(locale);
    if !known {
        crate::diagnostics::report_full(
            world,
            format!("locale_without_strings:{locale}"),
            Diagnostic::new(
                code::LOCALE_WITHOUT_STRINGS,
                format!(
                    "the locale `{locale}` was set, but no `kind: strings` doc is written in it"
                ),
            )
            .with_help("add a `kind: strings` doc for the locale; keys fall back to the default"),
        );
    }
    let id = state_entity(world);
    if let Ok(Some(state)) = world.get_mut::<LocaleState>(id) {
        let next = Some(locale.to_owned());
        if state.locale != next {
            state.locale = next;
        }
    }
}

/// The active locale: the one [`set_locale`] set, else the default locale.
pub fn locale(world: &mut World) -> String {
    active_locale(world)
}

/// [`locale`] for a read-only world: extract and hit tests hold `&World`.
///
/// ```
/// use ludion_ecs::World;
/// use ludion_engine::localization;
///
/// assert_eq!(localization::active_locale(&World::new()), "en");
/// ```
#[must_use]
pub fn active_locale(world: &World) -> String {
    world
        .entity_indices_with(std::any::TypeId::of::<LocaleState>())
        .find_map(|slot| world.live_id_at(slot))
        .and_then(|id| world.get::<LocaleState>(id).ok().flatten())
        .and_then(|state| state.locale.clone())
        .unwrap_or_else(|| default_locale(world))
}

/// Sets the `{name}` placeholder values of `entity`'s keyed [`UiText`] and [`Text`]. Returns whether
/// the entity has either.
///
/// # Example
/// ```
/// use ludion_components::UiText;
/// use ludion_engine::{Engine, localization};
///
/// let mut engine = Engine::builder().localization().build().unwrap();
/// let world = engine.world_mut();
/// let label = world.spawn();
/// world.insert(label, UiText { key: Some("hud.ammo".into()), ..Default::default() }).unwrap();
/// assert!(localization::set_text_args(world, label, &[("count", "12")]));
/// ```
pub fn set_text_args(world: &mut World, entity: EntityId, args: &[(&str, &str)]) -> bool {
    let next: std::collections::BTreeMap<String, String> = args
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let mut found = false;
    if let Ok(Some(ui)) = world.get_mut::<UiText>(entity) {
        found = true;
        if ui.args != next {
            ui.args.clone_from(&next);
        }
    }
    if let Ok(Some(text)) = world.get_mut::<Text>(entity) {
        found = true;
        if text.args != next {
            text.args = next;
        }
    }
    found
}

/// What one keyed (or formerly keyed) text needs resolved.
struct Job {
    id: EntityId,
    key: Option<String>,
    args: Vec<(String, String)>,
    resolved: Option<String>,
}

fn args_of(args: &std::collections::BTreeMap<String, String>) -> Vec<(String, String)> {
    args.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Resolves every keyed `UiText` and `Text` into its `resolved` field, in `EntityId` order (`UiText`
/// first), and clears `resolved` where the key is gone. `text` is never written. A no-op without
/// `.localization()`.
pub fn localize_text(world: &mut World, _ctx: &SystemCtx) {
    resolve_pass(world);
}

/// One resolve pass; also run after every world rebuild or restore so the first extract shows the
/// resolved text.
pub(crate) fn resolve_pass(world: &mut World) {
    if world.resource::<LocalizationEnabled>().is_none() {
        return;
    }
    let mut ui: Vec<Job> = world
        .query_ids::<(&UiText,)>()
        .filter(|(_, (t,))| t.key.is_some() || t.resolved.is_some())
        .map(|(id, (t,))| Job {
            id,
            key: t.key.clone(),
            args: args_of(&t.args),
            resolved: t.resolved.clone(),
        })
        .collect();
    ui.sort_by_key(|job| job.id);
    for job in ui {
        let next = resolved_for(world, &job);
        if next != job.resolved
            && let Ok(Some(t)) = world.get_mut::<UiText>(job.id)
        {
            t.resolved = next;
        }
    }
    let mut texts: Vec<Job> = world
        .query_ids::<(&Text,)>()
        .filter(|(_, (t,))| t.key.is_some() || t.resolved.is_some())
        .map(|(id, (t,))| Job {
            id,
            key: t.key.clone(),
            args: args_of(&t.args),
            resolved: t.resolved.clone(),
        })
        .collect();
    texts.sort_by_key(|job| job.id);
    for job in texts {
        let next = resolved_for(world, &job);
        if next != job.resolved
            && let Ok(Some(t)) = world.get_mut::<Text>(job.id)
        {
            t.resolved = next;
        }
    }
}

fn resolved_for(world: &mut World, job: &Job) -> Option<String> {
    let key = job.key.as_deref()?;
    let args: Vec<(&str, &str)> = job
        .args
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    Some(render(world, key, &args))
}

/// `L5139` once per engine when a keyed `UiText` or `Text` exists but the engine did not opt in to
/// localization, so the authored `text` is what draws.
pub(crate) fn report_unopted_keys(world: &mut World) {
    if world.resource::<LocalizationEnabled>().is_some() {
        return;
    }
    let keyed = world
        .query_ids::<(&UiText,)>()
        .any(|(_, (t,))| t.key.is_some())
        || world
            .query_ids::<(&Text,)>()
            .any(|(_, (t,))| t.key.is_some());
    if keyed {
        crate::diagnostics::report_full(
            world,
            "localization_not_enabled",
            Diagnostic::new(
                code::LOCALIZATION_NOT_ENABLED,
                "a `UiText` or `Text` has a `key`, but the engine did not opt in to localization; its authored `text` is drawn",
            )
            .with_help("add `.localization()` to the engine builder; see `ludion docs localization`"),
        );
    }
}
