//! Isolate one platform (POLA-52): everything except the platform, the platforms stacked on it
//! and their nodes is hidden. Hidden entities are skipped by picking's ray cast.

use std::collections::BTreeSet;

use bevy::{camera::visibility::VisibilitySystems, prelude::*};
use lab_core::{Document, PlatformId, Support};

use crate::host::{EventSender, PendingRequests};
use crate::scene::SceneIndex;
use crate::{FrameRequest, ViewportEvent};

#[derive(Resource, Default)]
pub(crate) struct Isolated(pub Option<PlatformId>);

pub(crate) struct IsolatePlugin;

impl Plugin for IsolatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Isolated>()
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
    if !dirty && isolated.0.is_none() {
        return;
    }
    let (Some(doc), kept) = (index.snapshot.as_ref(), isolated.0) else { return };
    let kept = kept.map(|root| kept_platforms(doc, root));
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
    for (&id, platform) in &index.platforms {
        set(platform.root, show(Some(id)));
    }
    for (&id, &entity) in &index.nodes {
        set(entity, show(doc.root_platform(id)));
    }
}
