//! Feature detection for the live commands, supplied by the app (the bin owns the mesh cache).

use lab_core::part::{Feature, PartDef};
use lab_core::Node;

pub trait FeatureLookup {
    fn detected(&self, node: &Node, def: &PartDef) -> Vec<Feature>;
}
