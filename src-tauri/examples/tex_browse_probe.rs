//! Exercise the browsable catalog + thumbnail decode against a real `vz.wad`.
//!
//! `cargo run --example tex_browse_probe -- "<game root>" [query]`

use mercs2_modkit_lib::commands::texture_swap::{list_textures, texture_previews};

fn main() {
    let game = std::env::args().nth(1).expect("usage: <game root> [query]");
    let query = std::env::args().nth(2).unwrap_or_default();

    let all = list_textures(game.clone()).expect("catalog");
    println!("catalog: {} named textures", all.len());

    let mut cats: std::collections::BTreeMap<&str, usize> = Default::default();
    for t in &all {
        *cats.entry(t.category.as_str()).or_default() += 1;
    }
    let mut top: Vec<_> = cats.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!(
        "top groups: {}",
        top.iter()
            .take(10)
            .map(|(c, n)| format!("{c}({n})"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // Mirrors the UI's ranking: every token must match, and word-boundary hits outrank
    // ones buried mid-word (a plain substring search for "eva" returns
    // `al_veh_truck_mtv_expandabl(e_va)n`, which is noise).
    let toks: Vec<String> = query.split_whitespace().map(|s| s.to_lowercase()).collect();
    let score_token = |words: &[&str], name: &str, tok: &str| -> i64 {
        if words.iter().any(|w| *w == tok) {
            4
        } else if words.iter().any(|w| w.starts_with(tok)) {
            3
        } else if words.iter().any(|w| w.contains(tok)) {
            2
        } else if name.contains(tok) {
            1
        } else {
            0
        }
    };

    let mut scored: Vec<(i64, &_)> = Vec::new();
    for t in &all {
        let name = t.name.to_lowercase();
        let words: Vec<&str> = name.split('_').collect();
        let mut total = 0i64;
        let mut ok = true;
        for tok in &toks {
            let s = score_token(&words, &name, tok);
            if s == 0 {
                ok = false;
                break;
            }
            total += s;
        }
        if ok {
            scored.push((total * 100 - name.len() as i64, t));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

    let hits: Vec<_> = scored.iter().take(8).map(|(_, t)| *t).collect();
    println!("\nquery {query:?} -> {} matched, top:", scored.len());

    let names: Vec<String> = hits.iter().map(|t| t.name.clone()).collect();
    let previews = texture_previews(game, names, None).expect("previews");
    for p in &previews {
        // A real decode, not a placeholder: check the PNG magic survived base64.
        assert!(p.data_url.starts_with("data:image/png;base64,iVBORw"));
        println!(
            "  {:<34} {}x{}  preview {}x{}  ({} B data-url)",
            p.name,
            p.width,
            p.height,
            p.preview_width,
            p.preview_height,
            p.data_url.len()
        );
    }
    println!("\nOK — {} thumbnails decoded to real PNGs.", previews.len());
}
