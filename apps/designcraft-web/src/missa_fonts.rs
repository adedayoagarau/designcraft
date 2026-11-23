//! Missa: the typefaces a document opened from Missa needs, from Missa's own font files.
//!
//! Missa serves TrueType copies of its writing typefaces next to the editor, at `fonts/`, with
//! `fonts/manifest.json` naming each family's files (`{"families": {"Literata": ["literata.ttf"]}}`).
//! Before a document opens, the editor reads which families it uses and lacks, and fetches only
//! those, so the text is set in the faces the writers chose and the Missing Fonts dialog doesn't
//! open for faces Missa has.

use designcraft_engine::Session;
use serde_json::{Value, json};

/// The most files one document may fetch, so a hostile manifest can't make the editor download
/// without end.
pub(crate) const MAX_FILES: usize = 24;

/// The families `bytes` (a document named `name`) sets text in that the editor has no face for,
/// read by opening it in a session of its own. A file that doesn't open needs nothing: opening
/// it for real reports why.
pub(crate) fn missing_families(name: &str, bytes: &[u8]) -> Vec<String> {
    let mut scratch = Session::new();
    let opened = scratch.execute("file.openBytes", &json!({"name": name, "base64": designcraft_engine::cmd::base64_encode(bytes)}));
    if opened.is_err() {
        return Vec::new();
    }
    let Ok(Value::Array(fonts)) = scratch.execute("font.list", &json!({})) else { return Vec::new() };
    let mut families: Vec<String> = Vec::new();
    for font in fonts {
        if font["missing"] == true
            && let Some(family) = font["family"].as_str()
            && !families.iter().any(|f| f.eq_ignore_ascii_case(family))
        {
            families.push(family.to_string());
        }
    }
    families
}

/// The files under `fonts/` that set `families`, from the manifest: only plain `.ttf` or `.otf`
/// names in that folder, each once, at most [`MAX_FILES`].
pub(crate) fn files_for(manifest: &Value, families: &[String]) -> Vec<String> {
    let Some(known) = manifest["families"].as_object() else { return Vec::new() };
    let mut files: Vec<String> = Vec::new();
    for family in families {
        let Some((_, list)) = known.iter().find(|(name, _)| name.eq_ignore_ascii_case(family)) else { continue };
        for file in list.as_array().into_iter().flatten().filter_map(Value::as_str) {
            if plain_font_file(file) && !files.iter().any(|f| f == file) && files.len() < MAX_FILES {
                files.push(file.to_string());
            }
        }
    }
    files
}

fn plain_font_file(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    !stem.is_empty()
        && stem.len() <= 64
        && stem.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && matches!(ext, "ttf" | "otf")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn document_in(family: &str) -> Vec<u8> {
        let mut s = Session::new();
        s.execute("file.new", &json!({})).unwrap();
        s.execute("frame.create", &json!({"rect": [72, 72, 300, 200], "content": "text", "text": "salt"})).unwrap();
        let d = std::sync::Arc::make_mut(&mut s.doc_mut().unwrap().doc);
        let sid = *d.stories.keys().next().unwrap();
        d.story_mut(sid).unwrap().format_chars(0..4, |f| f.over.font_family = Some(family.into()));
        let native = s.execute("file.serialize", &json!({})).unwrap();
        designcraft_engine::cmd::base64_decode(native["base64"].as_str().unwrap())
    }

    #[test]
    fn a_document_names_only_the_faces_the_editor_lacks() {
        let missing = missing_families("Issue.designcraft", &document_in("A Face Missa Has"));
        assert_eq!(missing, ["A Face Missa Has"]);
        assert!(missing_families("Issue.designcraft", &document_in("Source Serif 4")).is_empty(), "a bundled face is not fetched");
        assert!(missing_families("Issue.idml", b"not a document").is_empty());
    }

    #[test]
    fn the_manifest_gives_plain_files_for_the_families_asked_for() {
        let manifest = json!({"families": {
            "Literata": ["literata.ttf"],
            "Courier Prime": ["courier-prime.ttf", "courier-prime-bold.ttf"],
            "Bad": ["../secret.ttf", "/etc/x.ttf", "https://evil.example/a.ttf", "page.html", "Upper.ttf", "a.ttf?x"],
        }});
        let asked = |families: &[&str]| files_for(&manifest, &families.iter().map(|f| f.to_string()).collect::<Vec<_>>());
        assert_eq!(asked(&["literata", "Courier Prime"]), ["literata.ttf", "courier-prime.ttf", "courier-prime-bold.ttf"]);
        assert!(asked(&["Bad", "Unknown"]).is_empty());
        let many = json!({"families": {"Big": (0..100).map(|i| format!("f{i}.ttf")).collect::<Vec<_>>()}});
        assert!(files_for(&many, &["Big".into()]).len() <= MAX_FILES);
    }
}
