//! Imported parts (`Geometry::Mesh`): stored GLBs loaded through the `user://` and `builtin://`
//! asset sources, one child per LOD with a [`VisibilityRange`] (M4-PLAN POLA-58, DESIGN §13).
//!
//! Loads are asynchronous. The host polls the main schedule (no rendering) while [`GlbLoading`]
//! is set, and the asset events below then schedule the frames that show the loaded part.

use std::f32::consts::FRAC_PI_2;

use bevy::{
    asset::LoadState,
    gltf::{Gltf, GltfAssetLabel},
    camera::visibility::VisibilityRange,
    prelude::*,
    world_serialization::{WorldAssetRoot, WorldInstanceReady},
};
use lab_catalog::Source;
use lab_core::{NodeId, SelectionTarget};

use crate::host::SettleFrames;
use crate::picking::{HighlightLook, PickId, SharedMaterial};
use crate::scene::{MM, SceneEntity, SceneNode};

/// Distance bands (render metres) per LOD, with crossfade margins.
const LOD_RANGES: [(std::ops::Range<f32>, std::ops::Range<f32>); 3] = [
    (0.0..0.0, 1.4..1.6),
    (1.4..1.6, 3.8..4.2),
    (3.8..4.2, f32::MAX..f32::MAX),
];

/// One LOD scene of a node's GLB, waiting for (or past) its `WorldInstanceReady`.
#[derive(Component)]
pub(crate) struct GlbLod {
    node: NodeId,
    owner: SelectionTarget,
    pickable: bool,
    /// LOD0: the only LOD that carries `SceneNode`/`PickId`, so outline, picking and framing
    /// see one copy of the part (POLA-58).
    primary: bool,
    ready: bool,
}

/// True while some GLB scene has not spawned yet; read by the host loop.
#[derive(Resource, Default)]
pub(crate) struct GlbLoading(pub bool);

pub(crate) struct GlbPlugin;

impl Plugin for GlbPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GlbLoading>()
            .add_observer(tag_instance)
            .add_systems(Update, (wake_on_asset_events, track_loading));
    }
}

/// Registers `user://` and `builtin://`; call before the asset plugin is built.
pub(crate) fn register_sources(app: &mut App) {
    use bevy::asset::{AssetApp, io::AssetSourceBuilder};
    if let Some(user) = lab_catalog::user_dir() {
        if let Err(e) = std::fs::create_dir_all(&user) {
            warn!("could not create the user library {}: {e}", user.display());
        }
        app.register_asset_source("user", AssetSourceBuilder::platform_default(&user.to_string_lossy(), None));
    }
    let builtin = lab_catalog::builtin_dir();
    app.register_asset_source("builtin", AssetSourceBuilder::platform_default(&builtin.to_string_lossy(), None));
}

/// Spawns the LOD children of `parent` for GLB `asset`. A missing file shows nothing.
pub(crate) fn spawn_glb(
    commands: &mut Commands,
    assets: &AssetServer,
    parent: Entity,
    asset: &str,
    node: NodeId,
    pickable: bool,
) -> Result<(), String> {
    let (source, _) = lab_catalog::mesh_path(asset).ok_or_else(|| format!("its mesh file {asset} is missing"))?;
    let hex = &asset["blake3:".len()..];
    let src = match source {
        Source::User => "user",
        Source::BuiltIn => "builtin",
    };
    for (i, (start, end)) in LOD_RANGES.into_iter().enumerate() {
        let path = format!("{src}://meshes/{hex}.glb");
        commands.spawn((
            ChildOf(parent),
            GlbLod { node, owner: SelectionTarget::Node(node), pickable, primary: i == 0, ready: false },
            WorldAssetRoot(assets.load(GltfAssetLabel::Scene(i).from_asset(path))),
            // GLB vertices are part space (mm, +Z up); glTF coordinate conversion is off.
            Transform { rotation: Quat::from_rotation_x(-FRAC_PI_2), scale: Vec3::splat(MM), ..default() },
            VisibilityRange { start_margin: start, end_margin: end, ..default() },
        ));
    }
    Ok(())
}

/// Tags the spawned meshes like generated pieces so picking, highlight, outline and isolate
/// treat them alike. Instances of one GLB share its materials; highlights swap to tinted copies.
fn tag_instance(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    mut lods: Query<&mut GlbLod>,
    children: Query<&Children>,
    pieces: Query<&MeshMaterial3d<StandardMaterial>, With<Mesh3d>>,
    materials: Res<Assets<StandardMaterial>>,
    mut settle: ResMut<SettleFrames>,
) {
    let Ok(mut lod) = lods.get_mut(ready.entity) else { return };
    lod.ready = true;
    for entity in children.iter_descendants(ready.entity) {
        let Ok(material) = pieces.get(entity) else { continue };
        let Some(base_color) = materials.get(&material.0).map(|m| m.base_color) else { continue };
        let mut piece = commands.entity(entity);
        piece.insert((
            SceneEntity,
            HighlightLook::Standard { material: material.0.clone(), base_color, owner: lod.owner },
            SharedMaterial,
        ));
        if lod.primary {
            piece.insert(SceneNode(lod.node));
        }
        if lod.pickable && lod.primary {
            piece.insert(PickId(lod.owner));
        }
    }
    settle.0 = settle.0.max(2);
}

/// Loaded assets reach the render world only in a rendered frame: render two after each.
fn wake_on_asset_events(
    mut gltf: MessageReader<AssetEvent<Gltf>>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
    mut settle: ResMut<SettleFrames>,
) {
    let loaded = gltf.read().any(|e| matches!(e, AssetEvent::LoadedWithDependencies { .. }))
        | meshes.read().any(|e| matches!(e, AssetEvent::LoadedWithDependencies { .. }));
    if loaded {
        settle.0 = settle.0.max(2);
    }
}

/// Updates [`GlbLoading`]; a scene that failed to load is dropped with a warning.
fn track_loading(
    mut commands: Commands,
    lods: Query<(Entity, &GlbLod, &WorldAssetRoot)>,
    server: Res<AssetServer>,
    mut loading: ResMut<GlbLoading>,
) {
    let mut any = false;
    for (entity, lod, root) in &lods {
        if lod.ready {
            continue;
        }
        if let Some(LoadState::Failed(e)) = server.get_load_state(&root.0) {
            warn!("an imported part could not be loaded: {e}");
            commands.entity(entity).try_despawn();
        } else {
            any = true;
        }
    }
    loading.0 = any;
}
