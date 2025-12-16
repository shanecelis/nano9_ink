use super::*;
use bevy_mod_scripting::{
    GetTypeDependencies,
    bindings::{
        AppReflectAllocator, InteropError, IntoScriptRef, ReflectReference,
        WorldAccessGuard,
        function::from::Val,
        script_value::ScriptValue,
    },
    lua::{LuaScriptingPlugin, mlua::UserData},
    prelude::{ScriptCallbackEvent, callback_labels, event_handler},
};

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<InkStoryRef>()
        .add_systems(Update, on_reload_eval_func.after(hot_reload_on_modify));
    lua::plugin(app);
}

#[derive(Debug, Clone, Copy, Reflect, GetTypeDependencies)]
pub struct InkStoryRef(pub Entity);

impl InkStoryRef {
    pub fn into_script_ref(self, world: WorldAccessGuard) -> Result<ScriptValue, InteropError> {
        let reference = {
            let allocator = world.allocator();
            let mut allocator = allocator.write();
            ReflectReference::new_allocated(self, &mut allocator)
        };
        ReflectReference::into_script_ref(reference, world)
    }
}

impl UserData for InkStoryRef {}

fn on_reload_eval_func(
    mut events: EventReader<InkEvent>,
    mut writer: EventWriter<ScriptCallbackEvent>,
    allocator: ResMut<AppReflectAllocator>,
) {
    // For each modified asset, rebuild the runtime for all referencing entities.
    for ev in events.read() {
        match ev {
            InkEvent::OnStoryReload(id) => {
                let story_ref = InkStoryRef(*id);
                let mut allocator = allocator.write();
                let story_ref = ReflectReference::new_allocated(story_ref, &mut allocator);

                writer.write(ScriptCallbackEvent::new_for_all_scripts(
                    OnStoryReload,
                    vec![story_ref.into()],
                ));
            }
        }
    }
}

callback_labels!(OnStoryReload => "on_story_reload");

mod lua {
    use super::*;

    use bevy_mod_scripting::bindings::{
        ReflectAccessId,
        function::{
            namespace::{GlobalNamespace, NamespaceBuilder},
            script_function::FunctionCallContext,
        },
    };
    pub(crate) fn plugin(app: &mut App) {
        app.add_systems(
            PostUpdate,
            event_handler::<OnStoryReload, LuaScriptingPlugin>,
        );
        let world = app.world_mut();

        NamespaceBuilder::<GlobalNamespace>::new_unregistered(world).register(
            "ink_load",
            |ctx: FunctionCallContext, path: String| -> Result<ScriptValue, InteropError> {
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
                        stories
                            .get(this.0.0)
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
                        stories.get(this.0.0).is_ok()
                    })
                },
            )
            .register(
                "get_current_choices",
                |ctx: FunctionCallContext,
                 this: Val<InkStoryRef>|
                 -> Result<Vec<String>, InteropError> {
                    let world = ctx.world()?;
                    world.with_global_access(|world| {
                        let stories = world.non_send_resource::<InkStories>();
                        stories
                            .get(this.0.0)
                            .map(|story| {
                                story
                                    .get_current_choices()
                                    .iter()
                                    .map(|choice| choice.text.clone())
                                    .collect()
                            })
                            .map_err(|e| InteropError::external(Box::new(e)))
                    })?
                },
            )
            .register(
                "choose_choice_index",
                |ctx: FunctionCallContext,
                 this: Val<InkStoryRef>,
                 index: usize|
                 -> Result<(), InteropError> {
                    let world = ctx.world()?;
                    world.with_global_access(|world| {
                        let mut stories = world.non_send_resource_mut::<InkStories>();
                        stories
                            .get_mut(this.0.0)
                            .and_then(|story| {
                                story.choose_choice_index(index).map_err(InkError::from)
                            })
                            .map_err(|e| InteropError::external(Box::new(e)))
                    })?
                },
            )
            .register(
                "cont",
                |ctx: FunctionCallContext,
                 this: Val<InkStoryRef>|
                 -> Result<String, InteropError> {
                    let world = ctx.world()?;
                    world.with_global_access(|world| {
                        let mut stories = world.non_send_resource_mut::<InkStories>();
                        stories
                            .get_mut(this.0.0)
                            .and_then(|story| story.cont().map_err(InkError::from))
                            .map_err(|e| InteropError::external(Box::new(e)))
                    })?
                },
            );
    }
}
