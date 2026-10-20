//! The small GLB subset step2glb writes and LabSetup stores: triangle primitives with
//! POSITION/NORMAL, u8/u16/u32 indices, `baseColorFactor` materials, node TRS or matrix.

use glam::{DMat3, DMat4, DQuat, DVec2, DVec3};
use lab_geom::TriMesh;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
}

/// One primitive of a scene, transformed into scene space. `material` is `None` when the
/// primitive has none (AP203 files without colours).
pub struct Prim {
    pub material: Option<Material>,
    pub mesh: TriMesh,
}

pub struct Glb {
    json: Value,
    bin: Vec<u8>,
}

impl Glb {
    pub fn parse(bytes: &[u8]) -> Result<Glb, String> {
        let u32_at = |o: usize| -> Result<usize, String> {
            bytes
                .get(o..o + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                .ok_or_else(|| "file is truncated".to_string())
        };
        if bytes.get(0..4) != Some(b"glTF") {
            return Err("not a GLB file".into());
        }
        let json_len = u32_at(12)?;
        let json_bytes = bytes.get(20..20 + json_len).ok_or("JSON chunk is truncated")?;
        let json: Value = serde_json::from_slice(json_bytes).map_err(|e| e.to_string())?;
        let bin_at = 20 + json_len;
        let bin = if bytes.len() >= bin_at + 8 {
            let len = u32_at(bin_at)?;
            bytes.get(bin_at + 8..bin_at + 8 + len).ok_or("binary chunk is truncated")?.to_vec()
        } else {
            Vec::new()
        };
        Ok(Glb { json, bin })
    }

    pub fn scene_count(&self) -> usize {
        self.json["scenes"].as_array().map_or(0, Vec::len)
    }

    /// The first node or mesh name, if any.
    pub fn first_name(&self) -> Option<String> {
        ["nodes", "meshes"].iter().find_map(|k| {
            self.json[k].as_array()?.iter().find_map(|n| n["name"].as_str().filter(|s| !s.is_empty()))
        }).map(str::to_string)
    }

    /// Every primitive of scene `scene`, with node transforms applied.
    pub fn primitives(&self, scene: usize) -> Result<Vec<Prim>, String> {
        let roots = self.json["scenes"][scene]["nodes"].as_array().ok_or(format!("no scene {scene}"))?;
        let mut out = Vec::new();
        for r in roots {
            self.node(idx(r)?, DMat4::IDENTITY, &mut out, 0)?;
        }
        Ok(out)
    }

    fn node(&self, i: usize, parent: DMat4, out: &mut Vec<Prim>, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("node hierarchy is too deep".into());
        }
        let n = &self.json["nodes"][i];
        let local = if let Some(m) = n["matrix"].as_array() {
            let a: Vec<f64> = m.iter().filter_map(Value::as_f64).collect();
            DMat4::from_cols_slice(a.get(..16).ok_or("bad node matrix")?)
        } else {
            let v = |k: &str, d: &[f64]| -> Vec<f64> {
                n[k].as_array().map_or(d.to_vec(), |a| a.iter().filter_map(Value::as_f64).collect())
            };
            let (t, r, s) = (v("translation", &[0.0; 3]), v("rotation", &[0., 0., 0., 1.]), v("scale", &[1.0; 3]));
            if t.len() < 3 || r.len() < 4 || s.len() < 3 {
                return Err("bad node transform".into());
            }
            DMat4::from_scale_rotation_translation(
                DVec3::new(s[0], s[1], s[2]),
                DQuat::from_xyzw(r[0], r[1], r[2], r[3]),
                DVec3::new(t[0], t[1], t[2]),
            )
        };
        let world = parent * local;
        if let Some(m) = n.get("mesh") {
            let normal_m = DMat3::from_mat4(world).inverse().transpose();
            for p in self.json["meshes"][idx(m)?]["primitives"].as_array().ok_or("mesh without primitives")? {
                if p["mode"].as_u64().unwrap_or(4) != 4 {
                    continue; // only triangle lists
                }
                let pos = self.vec3(idx(&p["attributes"]["POSITION"])?)?;
                let nrm = match p["attributes"].get("NORMAL") {
                    Some(a) => self.vec3(idx(a)?)?,
                    None => vec![DVec3::Z; pos.len()],
                };
                let indices = match p.get("indices") {
                    Some(a) => self.indices(idx(a)?)?,
                    None => (0..pos.len() as u32).collect(),
                };
                if nrm.len() != pos.len() || indices.iter().any(|&k| k as usize >= pos.len()) {
                    return Err("primitive indices out of range".into());
                }
                let mesh = TriMesh {
                    positions: pos.iter().map(|&q| world.transform_point3(q)).collect(),
                    normals: nrm.iter().map(|&q| (normal_m * q).normalize_or(DVec3::Z)).collect(),
                    uvs: vec![DVec2::ZERO; pos.len()],
                    indices,
                };
                out.push(Prim { material: p.get("material").map(|m| self.material(m)).transpose()?, mesh });
            }
        }
        for c in n["children"].as_array().into_iter().flatten() {
            self.node(idx(c)?, world, out, depth + 1)?;
        }
        Ok(())
    }

    fn material(&self, m: &Value) -> Result<Material, String> {
        let pbr = &self.json["materials"][idx(m)?]["pbrMetallicRoughness"];
        let mut color = [1.0f32; 4];
        if let Some(c) = pbr["baseColorFactor"].as_array() {
            for (o, v) in color.iter_mut().zip(c) {
                *o = v.as_f64().unwrap_or(1.0) as f32;
            }
        }
        // step2glb writes colour only; glTF's default (metallic 1, rough 1) looks like raw
        // metal, so CAD colours without factors read as a satin paint.
        let f = |k: &str, d: f32| pbr[k].as_f64().map_or(d, |v| v as f32);
        Ok(Material { color, metallic: f("metallicFactor", 0.1), roughness: f("roughnessFactor", 0.5) })
    }

    /// (data slice start, element stride, count, componentType) of an accessor.
    fn view(&self, a: usize, elem: usize) -> Result<(usize, usize, usize, u64), String> {
        let acc = &self.json["accessors"][a];
        let count = acc["count"].as_u64().ok_or("accessor without count")? as usize;
        let ty = acc["componentType"].as_u64().unwrap_or(0);
        let bv = &self.json["bufferViews"][idx(&acc["bufferView"])?];
        let start = bv["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = bv["byteStride"].as_u64().map_or(elem, |s| s as usize);
        let end = if count == 0 { start } else { start + stride * (count - 1) + elem };
        if end > self.bin.len() {
            return Err("accessor runs past the binary chunk".into());
        }
        Ok((start, stride, count, ty))
    }

    fn vec3(&self, a: usize) -> Result<Vec<DVec3>, String> {
        let (start, stride, count, ty) = self.view(a, 12)?;
        if ty != 5126 {
            return Err("vertex data is not float".into());
        }
        let f = |o: usize| f32::from_le_bytes(self.bin[o..o + 4].try_into().unwrap()) as f64;
        Ok((0..count).map(|i| start + i * stride).map(|o| DVec3::new(f(o), f(o + 4), f(o + 8))).collect())
    }

    fn indices(&self, a: usize) -> Result<Vec<u32>, String> {
        let size = match self.json["accessors"][a]["componentType"].as_u64() {
            Some(5121) => 1,
            Some(5123) => 2,
            Some(5125) => 4,
            _ => return Err("unsupported index type".into()),
        };
        let (start, stride, count, _) = self.view(a, size)?;
        let b = &self.bin;
        Ok((0..count)
            .map(|i| start + i * stride)
            .map(|o| match size {
                1 => b[o] as u32,
                2 => u16::from_le_bytes([b[o], b[o + 1]]) as u32,
                _ => u32::from_le_bytes(b[o..o + 4].try_into().unwrap()),
            })
            .collect())
    }
}

fn idx(v: &Value) -> Result<usize, String> {
    v.as_u64().map(|i| i as usize).ok_or_else(|| "missing index".to_string())
}

/// Writes the stored layout: scene *i* = `lods[i]`, one root node with one mesh holding one
/// primitive per material. Materials are shared by index across LODs; `doubleSided` is false.
pub fn write(materials: &[Material], lods: &[Vec<(usize, TriMesh)>]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors) = (Vec::new(), Vec::new());
    let mut push = |data: Vec<u8>, target: u32, acc: Value| -> usize {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": data.len(), "target": target }));
        bin.extend(data);
        let mut acc = acc;
        acc["bufferView"] = json!(views.len() - 1);
        accessors.push(acc);
        accessors.len() - 1
    };
    let f32s = |v: &[DVec3]| v.iter().flat_map(|p| p.as_vec3().to_array()).flat_map(f32::to_le_bytes).collect::<Vec<u8>>();

    let (mut meshes, mut nodes, mut scenes) = (Vec::new(), Vec::new(), Vec::new());
    for (i, lod) in lods.iter().enumerate() {
        let prims: Vec<Value> = lod
            .iter()
            .map(|(mat, m)| {
                let (lo, hi) = crate::lod::bounds(&m.positions);
                let (lo, hi) = (lo.as_vec3().to_array(), hi.as_vec3().to_array());
                let n = m.positions.len();
                let pos = push(f32s(&m.positions), 34962, json!({ "componentType": 5126, "count": n, "type": "VEC3", "min": lo, "max": hi }));
                let nrm = push(f32s(&m.normals), 34962, json!({ "componentType": 5126, "count": n, "type": "VEC3" }));
                let ind = push(
                    m.indices.iter().flat_map(|i| i.to_le_bytes()).collect(),
                    34963,
                    json!({ "componentType": 5125, "count": m.indices.len(), "type": "SCALAR" }),
                );
                json!({ "attributes": { "POSITION": pos, "NORMAL": nrm }, "indices": ind, "material": mat, "mode": 4 })
            })
            .collect();
        meshes.push(json!({ "name": format!("lod{i}"), "primitives": prims }));
        nodes.push(json!({ "name": format!("lod{i}"), "mesh": i }));
        scenes.push(json!({ "nodes": [i] }));
    }
    let mats: Vec<Value> = materials
        .iter()
        .map(|m| json!({ "pbrMetallicRoughness": { "baseColorFactor": m.color, "metallicFactor": m.metallic, "roughnessFactor": m.roughness }, "doubleSided": false }))
        .collect();
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let doc = json!({
        "asset": { "version": "2.0", "generator": "LabSetup lab-import", "extras": { "labsetup_units": "mm", "labsetup_up_axis": "+Z" } },
        "scene": 0, "scenes": scenes, "nodes": nodes, "meshes": meshes, "materials": mats,
        "accessors": accessors, "bufferViews": views, "buffers": [{ "byteLength": bin.len() }],
    });
    let mut js = serde_json::to_vec(&doc).expect("JSON of plain values");
    while js.len() % 4 != 0 {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((js.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(js);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}
