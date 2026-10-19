//! Catalog part definitions (DESIGN.md §4.1). Pure data, shared by lab-catalog, lab-parts and
//! lab-mates. Part-local frame: upright, origin at the bottom centre (optics: centre), +Y optical axis.

use std::collections::BTreeMap;

use glam::DVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartDef {
    /// `vendor:part_number`, e.g. `thorlabs:KM100`, or `generic:<name>`.
    pub id: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub part_number: String,
    pub name: String,
    /// Slash path, e.g. `mounts/kinematic` (see the category list in docs/M2-PLAN.md).
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub geometry: Geometry,
    #[serde(default)]
    pub connectors: Vec<ConnectorDef>,
    #[serde(default)]
    pub optical_ports: Vec<OpticalPort>,
    #[serde(default)]
    pub meta: BTreeMap<String, String>,
}

fn default_version() -> u32 {
    1
}

impl PartDef {
    pub fn connector(&self, name: &str) -> Option<&ConnectorDef> {
        self.connectors.iter().find(|c| c.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    Parametric {
        generator: String,
        #[serde(default)]
        params: Params,
    },
    /// Content-addressed GLB (`sha256:…`), M4.
    Mesh { asset: String },
}

pub type Params = BTreeMap<String, ParamValue>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Number(f64),
    Text(String),
    Flag(bool),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Number(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConnectorDef {
    pub name: String,
    #[serde(flatten)]
    pub kind: ConnectorKind,
    pub frame: Frame,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConnectorKind {
    /// `spec`: "M4", "M6", "8-32", "1/4-20".
    Thread { spec: String, gender: Gender },
    /// Post in a holder. On the female side `range = [min, max]` bounds the slide (mm along the
    /// female frame's +Z; negative = deeper into the bore).
    Bore {
        diameter: f64,
        gender: Gender,
        #[serde(default)]
        range: Option<[f64; 2]>,
    },
    /// Optic in a mount: female = the mount's seat, male = the optic's edge.
    OpticBore { diameter: f64, gender: Gender },
    /// Bolts to a platform hole. Mates with platform holes, never with another connector.
    HoleGridSeat,
    CageRod { spacing: f64, gender: Gender },
    /// `thread`: "SM05", "SM1", "SM2".
    LensTube { thread: String, gender: Gender },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: DVec3,
    /// Mating axis, pointing out of the part. Unit length.
    pub z: DVec3,
    /// Twist reference, perpendicular to `z`. Unit length.
    pub x: DVec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpticalPort {
    pub name: String,
    /// "mirror", "lens", "splitter", "waveplate", "source", "detector", "dump", "fiber".
    pub kind: String,
    #[serde(default)]
    pub clear_aperture: Option<f64>,
    pub frame: Frame,
}
