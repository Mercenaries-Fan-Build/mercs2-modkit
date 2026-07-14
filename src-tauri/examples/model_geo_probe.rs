//! Does the 3D viewer's geometry + highlight actually resolve?
//!
//! `cargo run --release --example model_geo_probe -- "<game root>" <model> <texture>`

use mercs2_modkit_lib::commands::model_view::model_geometry;

fn main() {
    let mut a = std::env::args().skip(1);
    let game = a.next().expect("usage: <game root> <model> <texture>");
    let model = a.next().expect("model");
    let texture = a.next().expect("texture");

    // `None` tier = let the backend pick a state that actually shows the texture.
    let g = model_geometry(game, model.clone(), texture.clone(), None).expect("geometry");

    println!("model   {} (0x{:08X})", g.model, g.model_hash);
    println!(
        "verts   {}  tris {}  groups {}",
        g.positions.len() / 3,
        g.indices.len() / 3,
        g.groups.len()
    );
    println!(
        "bbox    [{:.2} {:.2} {:.2}] .. [{:.2} {:.2} {:.2}]",
        g.bbox_min[0], g.bbox_min[1], g.bbox_min[2], g.bbox_max[0], g.bbox_max[1], g.bbox_max[2]
    );
    println!(
        "\nhighlighted by \"{texture}\": {} of {} groups",
        g.highlighted_groups,
        g.groups.len()
    );
    for grp in &g.groups {
        let maps: Vec<String> = grp
            .textures
            .iter()
            .map(|s| {
                format!(
                    "{}{}={}",
                    if s.is_current { "*" } else { "" },
                    s.slot,
                    s.name.clone().unwrap_or(format!("0x{:08X}", s.hash))
                )
            })
            .collect();
        println!(
            "  [{:2}] prmg {:>2} node {:>3} lod {:#04x} tris {:>6}  {}  {}",
            grp.id,
            grp.prmg,
            grp.node,
            grp.lod_mask,
            grp.triangles,
            if grp.uses_texture { "**" } else { "  " },
            maps.join("  "),
        );
    }

    // Sanity: the arrays must be internally consistent or three.js will silently draw junk.
    assert_eq!(g.positions.len(), g.normals.len(), "one normal per position");
    assert_eq!(g.positions.len() / 3 * 2, g.uvs.len(), "one uv per vertex");
    let nverts = (g.positions.len() / 3) as u32;
    assert!(g.indices.iter().all(|&i| i < nverts), "every index in range");
    for grp in &g.groups {
        assert!(
            (grp.index_start + grp.index_count) as usize <= g.indices.len(),
            "group range inside the index buffer"
        );
    }
    // Optional: dump the geometry so it can be rendered in a real browser and eyeballed.
    // Numbers can't tell you the model is inside-out or the UVs are flipped; a picture can.
    if let Ok(out) = std::env::var("GEO_JSON") {
        std::fs::write(&out, serde_json::to_vec(&g).expect("serialize")).expect("write");
        println!("wrote {out}");
    }

    println!("\nOK — buffers are consistent and safe to hand to three.js.");
}
