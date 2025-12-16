use bevy::asset::{AssetEvent, AssetLoader, LoadContext, io::Reader};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use bladeink::{story::Story, story_error::StoryError};
use thiserror::Error;

#[cfg(feature = "scripting")]
pub mod scripting;

pub struct InkPlugin;

impl Plugin for InkPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<InkEvent>()
            .init_non_send_resource::<InkStories>()
            .init_asset::<InkText>()
            .init_asset_loader::<InkTextLoader>()
            .add_systems(Update, (load_on_add_then_poll, hot_reload_on_modify));
        #[cfg(feature = "scripting")]
        app.add_plugins(scripting::plugin);
    }
}

#[derive(Debug, Error)]
pub enum InkError {
    #[error("not loaded yet")]
    NotLoaded,
    #[error("no such story {0:?}")]
    NoSuchStory(Entity),
    #[error("story error: {0:?}")]
    StoryError(#[from] StoryError),
}

#[derive(Debug, Event, Clone)]
pub enum InkEvent {
    OnStoryReload(Entity),
}

#[derive(Default)]
pub struct InkStories(pub HashMap<Entity, Story>);

impl InkStories {
    /// Returns the prior story if there was one on success. Otherwise returns
    /// the error.
    pub fn try_parse(&mut self, id: Entity, ink: &InkText) -> Result<Option<Story>, StoryError> {
        Story::new(&ink.0).map(|story| self.0.insert(id, story))
    }

    pub fn get(&self, ink_story_ref: Entity) -> Result<&Story, InkError> {
        self.0.get(&ink_story_ref).ok_or(InkError::NotLoaded)
    }

    pub fn get_mut(&mut self, ink_story_ref: Entity) -> Result<&mut Story, InkError> {
        self.0.get_mut(&ink_story_ref).ok_or(InkError::NotLoaded)
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
    mut ink_stories: NonSendMut<InkStories>,
    // We need to re-fetch the handle while pending.
    ink_loads: Query<(Entity, &InkLoad)>,
    mut writer: EventWriter<InkEvent>,
) {
    // For each modified asset, rebuild the runtime for all referencing entities.
    for ev in events.read() {
        let asset_id = match ev {
            AssetEvent::Modified { id } => *id,
            AssetEvent::Removed { id: _ } => {
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
            if let Some(ink_text) = ink_texts.get(&ink.0) {
                match ink_stories.try_parse(entity, ink_text) {
                    Ok(_last_story) => {
                        writer.write(InkEvent::OnStoryReload(entity));
                    }
                    Err(err) => {
                        error!("Error parsing ink reload in {entity}: {err}");
                    }
                }
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
            match ink_stories.try_parse(e, ink) {
                Ok(_last_story) => {
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

#[derive(Default)]
pub struct InkTextLoader;

impl AssetLoader for InkTextLoader {
    type Asset = InkText;
    type Settings = ();
    type Error = std::io::Error;

    fn extensions(&self) -> &[&str] {
        &[
            "ink.json",
            #[cfg(feature = "inklecate")]
            "ink",
        ]
    }

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        // Check if the file extension is "ink" and compile it with inklecate
        let path = load_context.path();
        let extension = path.extension().and_then(|ext| ext.to_str());

        #[cfg(feature = "inklecate")]
        if extension == Some("ink") {
            use std::io::Write;
            use std::process::{Command, Stdio};

            let mut child = Command::new("inklecate")
                .args(["-o", "/dev/stdout", "/dev/stdin"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()?;

            child.stdin.as_mut().unwrap().write_all(&bytes)?;

            let output = child.wait_with_output()?;
            let compiled_json = String::from_utf8_lossy(&output.stdout);

            return Ok(InkText(compiled_json.into_owned()));
        }
        Ok(InkText(String::from_utf8_lossy(&bytes).into()))
    }
}
