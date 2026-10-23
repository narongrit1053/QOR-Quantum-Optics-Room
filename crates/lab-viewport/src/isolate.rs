//! Isolate one platform (POLA-52): everything except the platform, the platforms stacked on it
//! and their nodes is hidden. Hidden entities are skipped by picking's ray cast.

use std::collections::BTreeSet;

use bevy::{camera::visibility::VisibilitySystems, prelude::*};
use lab_core::{Document, NodeId, PlatformId, Support};

use crate::host::{EventSender, PendingRequests};
use crate::scene::SceneIndex;
use crate::{FrameRequest, ViewportEvent};

#[derive(Resource, Default)]
pub(crate) struct Isolated(pub Option<PlatformId>);

/// M9 D. Hidden nodes and the isolated selection, as sent (descendants are added when applied).
#[derive(Resource, Default)]
pub(crate) struct NodeView {
    hidden: Vec<NodeId>,
    isolated: Option<Vec<NodeId>>,
}

/// `ids` and all their descendants.
fn with_descendants(doc: &Document, ids: &[NodeId]) -> BTreeSet<NodeId> {
    ids.iter().flat_map(|&id| doc.descendants(id).into_iter().chain([id])).collect()
}

pub(crate) struct IsolatePlugin;

impl Plugin for IsolatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Isolated>()
            .init_resource::<NodeView>()
            .add_systems(PostUpdate, apply_isolation.before(VisibilitySystems::VisibilityPropagate));
    }
}

/// `root` and every platform whose `OnPlatform` chain reaches it.
pub(crate) fn kept_platforms(doc: &Document, root: PlatformId) -> BTreeSet<PlatformId> {
    let reaches = |mut id: PlatformId| {
        // Bounded walk: the document forbids loops, but do not trust it with a hang.
        for _ in 0..=doc.platforms.len() {
            if id == root {
                return true;
            }
            match doc.platform(id).map(|p| &p.support) {
                Some(Support::OnPlatform { parent, .. }) => id = *parent,
                _ => return false,
            }
        }
        false
    };
    doc.platforms.iter().map(|p| p.id).filter(|&id| reaches(id)).collect()
}

pub(crate) fn apply_isolation(
    pending: Res<PendingRequests>,
    index: Res<SceneIndex>,
    events: Res<EventSender>,
    mut isolated: ResMut<Isolated>,
    mut view: ResMut<NodeView>,
    mut visibility: Query<&mut Visibility>,
) {
    let exists = |id: PlatformId| index.snapshot.as_ref().is_some_and(|doc| doc.platform(id).is_some());
    let mut dirty = false;
    for request in &pending.0 {
        match request {
            FrameRequest::Isolate(id) => {
                let id = id.filter(|&id| exists(id));
                if id != isolated.0 || id.is_none() {
                    events.send(ViewportEvent::IsolationChanged(id));
                }
                isolated.0 = id;
                dirty = true;
            }
            FrameRequest::SetHidden(ids) => {
                view.hidden = ids.clone();
                dirty = true;
            }
            FrameRequest::IsolateNodes(ids) => {
                view.isolated = ids.clone();
                dirty = true;
            }
            FrameRequest::LoadDocument(_) | FrameRequest::ApplyChanges(..) => dirty = true,
            _ => {}
        }
    }
    if let Some(id) = isolated.0
        && !exists(id)
    {
        isolated.0 = None;
        events.send(ViewportEvent::IsolationChanged(None));
    }
    // While isolated, re-apply every frame: the wall fade writes wall visibility on its own.
    if !dirty && isolated.0.is_none() && view.isolated.is_none() {
        return;
    }
    let Some(doc) = index.snapshot.as_ref() else { return };
    let hidden = with_descendants(doc, &view.hidden);
    let only = view.isolated.as_ref().map(|ids| with_descendants(doc, ids));
    // Isolating nodes keeps their root platforms (not the platforms stacked on them).
    let kept = match &only {
        Some(only) => Some(only.iter().filter_map(|&id| doc.root_platform(id)).collect()),
        None => isolated.0.map(|root| kept_platforms(doc, root)),
    };
    let show = |platform: Option<PlatformId>| match (&kept, platform) {
        (None, _) => true,
        (Some(kept), Some(p)) => kept.contains(&p),
        (Some(_), None) => false,
    };
    let mut set = |entity: Entity, visible: bool| {
        let want = if visible { Visibility::Inherited } else { Visibility::Hidden };
        if let Ok(mut v) = visibility.get_mut(entity)
            && *v != want
        {
            *v = want;
        }
    };
    for &entity in &index.room {
        set(entity, kept.is_none());
    }
    // Nodes and platforms change only with the requests above; walking 10k nodes per frame does not.
    if !dirty {
        return;
    }
    for (&id, platform) in &index.platforms {
        set(platform.root, show(Some(id)));
    }
    for (&id, &entity) in &index.nodes {
        let visible = show(doc.root_platform(id)) && !hidden.contains(&id) && only.as_ref().is_none_or(|o| o.contains(&id));
        set(entity, visible);
    }
}
