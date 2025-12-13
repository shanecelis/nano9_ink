use bevy::prelude::*;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::asset::{AssetEvent, AssetLoader, LoadContext, io::Reader};
use bevy::reflect::TypeRegistry;
use bladeink::{story::Story, story_error::StoryError};
#[cfg(feature = "scripting")]
use bevy_mod_scripting::{
    GetTypeDependencies,
    lua::mlua::UserData,
    bindings::{
        IntoScriptRef,
        ReflectReference,
        ArgMeta,
        InteropError,
        docgen::typed_through::{ThroughTypeInfo, TypedThrough},
        script_value::ScriptValue,
        function::from::{Val, FromScript},
        IntoScript,
        WorldAccessGuard,
    }
};
use thiserror::Error;
use std::any::TypeId;

pub struct InkPlugin;

impl Plugin for InkPlugin {
    fn build(&self, app: &mut App) {
        app
            .register_type::<InkStoryRef>()
            .add_event::<InkEvent>()
            .init_non_send_resource::<InkStories>()
            .init_asset::<InkText>()
            .init_asset_loader::<InkTextLoader>()
            .add_systems(Update, (load_on_add_then_poll, hot_reload_on_modify));
        #[cfg(feature = "scripting")]
        lua::plugin(app);
    }
}

#[derive(Debug, Error)]
pub enum InkError {
    #[error("not loaded yet")]
    NotLoaded,
    #[error("no such story {0:?}")]
    NoSuchStory(InkStoryRef),
    #[error("story error: {0:?}")]
    StoryError(#[from] StoryError)
}

#[derive(Default)]
struct InkStories(HashMap<Entity, Story>);

impl InkStories {
    fn try_insert(&mut self, id: Entity, ink: &InkText) -> Result<Option<Story>, StoryError> {
        Story::new(&ink.0)
            .inspect(|_| { info!("inserted story"); })
            .map(|story| self.0.insert(id, story))
    }

    fn get(&self, ink_story_ref: &InkStoryRef) -> Result<&Story, InkError> {
        self.0.get(&ink_story_ref.0)
            .ok_or(InkError::NotLoaded)
    }

    fn get_mut(&mut self, ink_story_ref: &InkStoryRef) -> Result<&mut Story, InkError> {
        self.0.get_mut(&ink_story_ref.0)
            .ok_or(InkError::NotLoaded)
    }

}

#[derive(Debug, Component, Clone)]
pub struct InkLoad(pub Handle<InkText>);

#[derive(Debug, Component, Clone)]
pub struct InkStory;

#[derive(Debug, Asset, TypePath)]
pub struct InkText(pub String);

fn hot_reload_on_modify(
    ink_texts: Res<Assets<InkText>>,
    mut events: EventReader<AssetEvent<InkText>>,
    mut commands: Commands,
    mut ink_stories: NonSendMut<InkStories>,
    // We need to re-fetch the handle while pending.
    ink_loads: Query<(Entity, &InkLoad)>,
) {
    // For each modified asset, rebuild the runtime for all referencing entities.
    for ev in events.read() {
        let asset_id = match ev {
            AssetEvent::Modified { id } => *id,
            AssetEvent::Removed { id } => {
                // Optional: handle removal (e.g. remove InkRuntime from entities)
                // *id
                continue;
            }
            _ => continue,
        };
        for (entity, ink) in &ink_loads {
            if ink.0.id() != asset_id {
                continue;
            }
            info!("reloading ink on {entity}");
            if let Some(ink_text) = ink_texts.get(&ink.0)
            && let Err(err) = ink_stories.try_insert(entity, &ink_text) {
                error!("Error parsing ink reload in {entity}: {err}");
            }

        }
    }
}
pub fn load_on_add_then_poll(
    ink_texts: Res<Assets<InkText>>,
    mut commands: Commands,
    // Track only entities that *just gained* InkStory.
    added: Query<(Entity, &InkLoad), Added<InkLoad>>,
    mut ink_stories: NonSendMut<InkStories>,
    // We need to re-fetch the handle while pending.
    stories: Query<&InkLoad>,
    // Local set of entities waiting for their asset to become available.
    mut pending: Local<HashSet<Entity>>,
) {
    // Start tracking newly-added stories.
    for (e, _) in &added {
        pending.insert(e);
    }

    if pending.is_empty() {
        return;
    }

    // Poll pending entities; stop tracking when resolved.
    pending.retain(|&e| {
        let Ok(story) = stories.get(e) else {
            // Entity despawned or component removed.
            return false;
        };

        if let Some(ink) = ink_texts.get(&story.0) {
            match ink_stories.try_insert(e, ink) {
                Ok(last_story) => {
                    commands.entity(e).insert(InkStory);
                }
                Err(err) => {
                    error!("Error parsing ink in {e}: {err}");
                }
            }
            false // Remove from pending. Stop waiting.
        } else {
            true // Keep waiting.
        }
    });
}

#[derive(Debug, Clone, Copy, Reflect)]
#[cfg_attr(feature = "scripting", derive(GetTypeDependencies))]
pub struct InkStoryRef(pub Entity);

impl InkStoryRef {
    #[cfg(feature = "scripting")]
    pub fn into_script_ref(self, world: WorldAccessGuard) -> Result<ScriptValue, InteropError> {
        let reference = {
            let allocator = world.allocator();
            let mut allocator = allocator.write();
            ReflectReference::new_allocated(self, &mut allocator)
        };
        ReflectReference::into_script_ref(reference, world)
    }
}

#[cfg(feature = "scripting")]
impl UserData for InkStoryRef {}


#[derive(Default)]
pub struct InkTextLoader;

#[derive(Event)]
pub enum InkEvent {
    Load(Handle<InkText>),
}

impl AssetLoader for InkTextLoader {
    type Asset = InkText;
    type Settings = ();
    type Error = std::io::Error;

    fn extensions(&self) -> &[&str] {
        &["ink"]
    }

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(InkText(String::from_utf8_lossy(&bytes).into()))
    }
}

#[cfg(feature = "scripting")]
mod lua {
    use super::*;

    use bevy_mod_scripting::bindings::{
        ReflectAccessId,
        function::{
        namespace::{GlobalNamespace, NamespaceBuilder},
        script_function::FunctionCallContext,
    }};
    pub(crate) fn plugin(app: &mut App) {
        let world = app.world_mut();

        NamespaceBuilder::<GlobalNamespace>::new_unregistered(world).register(
            "ink_load",
            |ctx: FunctionCallContext,
             path: String| -> Result<ScriptValue, InteropError> {
                 let world_guard = ctx.world()?;
                 let raid = ReflectAccessId::for_global();
                 if world_guard.claim_global_access() {
                     let ink_story_ref = {
                         let world = world_guard.as_unsafe_world_cell()?;
                         let world = unsafe { world.world_mut() };
                         let ink_text = {
                             let asset_server = world.resource::<AssetServer>();
                             asset_server.load::<InkText>(&path)
                         };
                         let id = world.spawn(InkLoad(ink_text)).id();
                         InkStoryRef(id)
                     };
                     unsafe { world_guard.release_global_access() };
                     ink_story_ref.into_script_ref(world_guard)
                 } else {
                     Err(InteropError::cannot_claim_access(
                         raid,
                         world_guard.get_access_location(raid),
                         "ink_load",
                     ))
                 }
            },
        );

    NamespaceBuilder::<InkStoryRef>::new(app.world_mut())
        .register(
            "can_continue",
            |ctx: FunctionCallContext, this: Val<InkStoryRef>| -> Result<bool, InteropError> {
                let world = ctx.world()?;
                world.with_global_access(|world| {
                    let stories = world.non_send_resource::<InkStories>();
                    stories.get(&this)
                        .map(|story| story.can_continue())
                        .map_err(|e| InteropError::external(Box::new(e)))
                })?
            },
        )
        .register(
            "is_loaded",
            |ctx: FunctionCallContext, this: Val<InkStoryRef>| -> Result<bool, InteropError> {
                let world = ctx.world()?;
                world.with_global_access(|world| {
                    let stories = world.non_send_resource::<InkStories>();
                    stories.get(&this).is_ok()
                })
            },
        )
        .register(
            "get_current_choices",
            |ctx: FunctionCallContext, this: Val<InkStoryRef>| -> Result<Vec<String>, InteropError> {
                let world = ctx.world()?;
                world.with_global_access(|world| {
                    let stories = world.non_send_resource::<InkStories>();
                    stories.get(&this)
                        .map(|story| story.get_current_choices().iter().map(|choice| choice.text.clone()).collect())
                        .map_err(|e| InteropError::external(Box::new(e)))
                })?
            },
        )
        .register(
            "choose_choice_index",
            |ctx: FunctionCallContext, this: Val<InkStoryRef>, index: usize| -> Result<(), InteropError> {
                let world = ctx.world()?;
                world.with_global_access(|world| {
                    let mut stories = world.non_send_resource_mut::<InkStories>();
                    stories.get_mut(&this)
                        .and_then(|story| story.choose_choice_index(index).map_err(InkError::from))
                        .map_err(|e| InteropError::external(Box::new(e)))
                })?
            },
        )
        .register(
            "cont",
            |ctx: FunctionCallContext, this: Val<InkStoryRef>| -> Result<String, InteropError> {
                let world = ctx.world()?;
                world.with_global_access(|world| {
                    let mut stories = world.non_send_resource_mut::<InkStories>();
                    stories.get_mut(&this)
                        .and_then(|story| story.cont().map_err(InkError::from))
                        .map_err(|e| InteropError::external(Box::new(e)))
                })?
            },
        );
    }
}
