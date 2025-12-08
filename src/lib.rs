use bevy::prelude::*;
use bevy::asset::{AssetLoader, AsyncReadExt, LoadContext, LoadedAsset, io::Reader};
use bladeink::story::Story;

pub struct InkPlugin;

impl Plugin for InkPlugin {
    fn build(&self, app: &mut App) {
        app
            .init_non_send_resource::<InkStories>()
            .init_asset::<InkText>()
            .init_asset_loader::<InkTextLoader>();
        #[cfg(feature = "scripting")]
        lua::plugin(app);
    }
}

#[derive(Debug, Default)]
struct InkStories(Vec<InkStory>);

struct InkStory(Handle<InkText>, Option<Story>);

struct InkStoryRef { index: usize }


#[derive(Asset, TypePath)]
pub struct InkText(pub String);

#[derive(Default)]
pub struct InkTextLoader;

impl AssetLoader for InkTextLoader {
    type Asset = InkText;
    type Settings = ();
    type Error = std::io::Error;

    fn extensions(&self) -> &[&str] {
        &["txt"]
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
    use crate::pico8::lua::with_pico8;

    use bevy_mod_scripting::bindings::function::{
        namespace::{GlobalNamespace, NamespaceBuilder},
        script_function::FunctionCallContext,
    };
    pub(crate) fn plugin(app: &mut App) {
        let world = app.world_mut();

        NamespaceBuilder::<GlobalNamespace>::new_unregistered(world).register(
            "ink_load",
            |ctx: FunctionCallContext,
             path: String| {
                 let world_guard = ctx.world()?;
                 let raid = ReflectAccessId::for_global();
                 if world_guard.claim_global_access() {
                     let world = world_guard.as_unsafe_world_cell()?;
                     let world = unsafe { world.world_mut() };
                 } else {
                     Err(InteropError::cannot_claim_access(
                         raid,
                         world_guard.get_access_location(raid),
                         "with_system_param",
                     ))
                 }
                Ok(())
            },
        );
    }
}
