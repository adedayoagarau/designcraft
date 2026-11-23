//! Poem tools: set a text frame's own text on an arc, circle, wave or spiral (and back), and
//! split a frame into one frame per word or line, each where its text was set.

use std::sync::Arc;

use designcraft_doc::{Content, Document, Item, ItemId, PathType, Selection, Shape, Story, StoryId, TextFrame};
use designcraft_geom::{BezPath, PathData, Point, Rect, shapes};
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_selection, str_param, targets};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "poem.curve",
            "Curve Text",
            [],
            None,
            "{kind: none|arc|circle|wave|spiral, bend?: -100..100 (how far an arc or wave bows; negative bows down), ids?} — sets each selected text frame's own story on the curve, inside the frame's bounds; none sets it back in a rectangle",
            has_selection,
            curve
        ),
        cmd!(
            "poem.split",
            "Split Text Frame",
            [],
            None,
            "{by: words|lines, id?} → {ids} — one frame per word or line, each where it was set, with its formatting; the selected frame and its story go",
            has_selection,
            split
        ),
    ]
}

const CURVES: [&str; 5] = ["none", "arc", "circle", "wave", "spiral"];

fn curve(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "poem.curve";
    let kind = str_param(p, "kind").ok_or_else(|| bad(ID, "`kind` required"))?.to_string();
    if !CURVES.contains(&kind.as_str()) {
        return Err(bad(ID, format!("unknown curve `{kind}`")));
    }
    let bend = p.get("bend").and_then(Value::as_f64).unwrap_or(50.0).clamp(-100.0, 100.0);
    let ids = targets(s, p)?;
    s.edit(|d, _| {
        let mut changed = 0;
        for id in &ids {
            let sid = lone_story(d, *id, ID)?;
            let vertical = d.story(sid).is_some_and(|st| st.vertical);
            if vertical && kind != "none" {
                return Err(bad(ID, "set the story horizontal before curving it"));
            }
            let it = d.item_mut(*id).ok_or(designcraft_doc::DocError::NoItem(*id))?;
            let r = it.path.bounds().unwrap_or(Rect::ZERO);
            let (path, shape) = match kind.as_str() {
                "none" => (shapes::rectangle(r), Shape::Rectangle),
                "circle" => (shapes::ellipse(square_in(r)), Shape::Oval),
                k => (curve_path(k, r, bend), Shape::Path),
            };
            it.path = path;
            it.shape = shape;
            if let Content::Text(tf) = &mut it.content {
                tf.options.path = (kind != "none").then(PathType::default);
            }
            // An arc reads from its middle out, as a title set on an arc does.
            if kind == "arc"
                && let Some(st) = d.story_mut(sid)
            {
                let len = st.len();
                let mut res = Ok(());
                st.format_paras(0..len, |pf| {
                    if let Err(e) = pf.para.set_json("align", &json!("center")) {
                        res = Err(e);
                    }
                });
                res.map_err(|e| bad(ID, e))?;
            }
            changed += 1;
        }
        Ok(json!({"changed": changed}))
    })
}

/// The story of a text frame that holds it alone (not threaded, not inside a group).
fn lone_story(d: &Document, id: ItemId, cmd: &str) -> Result<StoryId> {
    let it = d.item(id).ok_or(designcraft_doc::DocError::NoItem(id))?;
    let tf = it.text_frame().ok_or_else(|| bad(cmd, "select a text frame"))?;
    let st = d.story(tf.story).ok_or(designcraft_doc::DocError::NoStory(tf.story))?;
    if st.frames.len() != 1 {
        return Err(bad(cmd, "the story runs through more than one frame; unthread it first"));
    }
    Ok(tf.story)
}

/// The largest square centred in `r`.
fn square_in(r: Rect) -> Rect {
    let side = r.width().min(r.height());
    Rect::from_center_size(r.center(), (side, side))
}

/// An open arc, wave or spiral inside `r`. Arcs and waves run left to right through the middle.
fn curve_path(kind: &str, r: Rect, bend: f64) -> PathData {
    let c = r.center();
    match kind {
        "arc" => {
            // The arc's rise is up to half the width; it bows up for a positive bend.
            let rise = (bend / 100.0) * (r.width() / 2.0).min(r.height().max(r.width() / 4.0));
            let base = c.y + rise / 2.0;
            let mut bp = BezPath::new();
            bp.move_to((r.x0, base));
            // A cubic whose two handles sit 4/3 of the rise up peaks at the rise.
            bp.curve_to((r.x0 + r.width() / 3.0, base - rise * 4.0 / 3.0), (r.x1 - r.width() / 3.0, base - rise * 4.0 / 3.0), (r.x1, base));
            PathData::from_bezpath(&bp)
        }
        "wave" => {
            let amp = (bend / 100.0) * (r.height() / 2.0).max(r.width() / 16.0);
            let n = 48;
            let pts: Vec<Point> = (0..=n)
                .map(|i| {
                    let t = i as f64 / n as f64;
                    Point::new(r.x0 + t * r.width(), c.y - amp * (t * std::f64::consts::TAU * 2.0).sin())
                })
                .collect();
            PathData::from_bezpath(&smooth(&pts))
        }
        _ => {
            // Spiral: from the outside in, so the poem starts where the letters have room.
            let mut sp = shapes::spiral(c, r.width().min(r.height()) / 2.0, 88.0, 14, true);
            for s in &mut sp.subpaths {
                s.reverse();
            }
            sp
        }
    }
}

/// A smooth curve through `pts` (Catmull-Rom as cubics).
fn smooth(pts: &[Point]) -> BezPath {
    let mut bp = BezPath::new();
    let Some(first) = pts.first() else { return bp };
    bp.move_to(*first);
    for i in 0..pts.len().saturating_sub(1) {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        bp.curve_to(p1 + (p2 - p0) / 6.0, p2 - (p3 - p1) / 6.0, p2);
    }
    bp
}

fn split(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "poem.split";
    let by_words = match str_param(p, "by").unwrap_or("words") {
        "words" => true,
        "lines" => false,
        o => return Err(bad(ID, format!("split by words or lines, not `{o}`"))),
    };
    let id = match targets(s, p)?.as_slice() {
        [one] => *one,
        _ => return Err(bad(ID, "select one text frame")),
    };
    s.edit(|d, sel| {
        let sid = lone_story(d, id, ID)?;
        let it = d.item(id).cloned().ok_or(designcraft_doc::DocError::NoItem(id))?;
        let loc = d.find(id).ok_or(designcraft_doc::DocError::NoItem(id))?;
        if loc.path.len() != 1 {
            return Err(bad(ID, "take the frame out of its group first"));
        }
        let st = d.story(sid).cloned().ok_or(designcraft_doc::DocError::NoStory(sid))?;
        if st.vertical || it.text_frame().is_some_and(|t| t.options.path.is_some()) {
            return Err(bad(ID, "only horizontal text in a frame can be split"));
        }
        if !st.tables.is_empty() || !st.anchors.is_empty() || !st.notes.is_empty() || !st.objects.is_empty() || !st.endnotes.is_empty() {
            return Err(bad(ID, "only plain text can be split; this story holds tables, notes or anchored objects"));
        }
        let cs = designcraft_compose::compose_story(d, sid, &Default::default());
        let ft = cs.frame(id).ok_or_else(|| bad(ID, "the frame shows no text"))?;
        // Each piece: its text range and where it was set, in the frame's text space.
        let mut pieces: Vec<(std::ops::Range<usize>, usize, Rect)> = Vec::new();
        for l in &ft.lines {
            let text = st.slice(l.range.clone());
            let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
            if by_words {
                let mut start = None;
                for (i, ch) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
                    match (start, ch.is_whitespace()) {
                        (None, false) => start = Some(i),
                        (Some(a), true) => {
                            spans.push(a..i);
                            start = None;
                        }
                        _ => {}
                    }
                }
            } else {
                let trimmed = text.trim_end();
                let lead = trimmed.len() - trimmed.trim_start().len();
                if trimmed.len() > lead {
                    spans.push(lead..trimmed.len());
                }
            }
            for sp in spans {
                let r = l.range.start + sp.start..l.range.start + sp.end;
                let (x0, x1) = (designcraft_compose::caret_x(l, r.start), designcraft_compose::caret_x(l, r.end));
                let rect = Rect::new(x0.min(x1), l.baseline - l.ascent, x0.max(x1), l.baseline + l.descent);
                pieces.push((r, l.para, rect));
            }
        }
        if pieces.len() < 2 {
            return Err(bad(ID, "there is only one piece to split"));
        }
        let text_xf = d.text_xf(&it);
        d.remove_item(id)?;
        let mut out = Vec::new();
        for (at, (range, para, rect)) in pieces.into_iter().enumerate() {
            let nid = ItemId(d.alloc());
            let nsid = StoryId(d.alloc());
            let mut pf = st.paras.get(para).cloned().unwrap_or_default();
            for (k, v) in [
                ("leftIndent", json!(0)),
                ("rightIndent", json!(0)),
                ("firstLineIndent", json!(0)),
                ("spaceBefore", json!(0)),
                ("spaceAfter", json!(0)),
                ("dropCapLines", json!(0)),
                ("align", json!("left")),
            ] {
                pf.para.set_json(k, &v).map_err(|e| bad(ID, e))?;
            }
            let mut piece = Story::with_text(nsid, st.slice(range.clone()), pf);
            for (run, fmt) in st.runs() {
                let (a, b) = (run.start.max(range.start), run.end.min(range.end));
                if a < b {
                    piece.format_chars(a - range.start..b - range.start, |f| *f = fmt.clone());
                }
            }
            piece.direction = st.direction;
            piece.optical_margin = st.optical_margin;
            piece.frames = vec![nid];
            d.stories.insert(nsid, Arc::new(piece));
            // A little room each side, so rounding never pushes the word out of its frame.
            let rect = rect.inflate(1.0, 0.5);
            let mut o = Item::new(nid, it.layer, Shape::Rectangle, shapes::rectangle(rect));
            o.xf = text_xf;
            let mut tf = TextFrame { story: nsid, options: Default::default() };
            tf.options.inset = [0.0; 4];
            o.content = Content::Text(tf);
            d.insert_item(loc.spread, o, Some(loc.path[0] + at))?;
            out.push(nid);
        }
        *sel = Selection::items(out.clone());
        Ok(json!({"ids": out.iter().map(|i| i.0).collect::<Vec<_>>()}))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(s: &mut Session, text: &str) -> ItemId {
        s.execute("file.new", &json!({})).unwrap();
        let r = s.execute("frame.create", &json!({"rect": [72, 72, 372, 272], "content": "text", "text": text})).unwrap();
        let id = ItemId(r["id"].as_u64().unwrap());
        s.execute("selection.set", &json!({"ids": [id.0]})).unwrap();
        id
    }

    fn story_text(d: &Document, id: ItemId) -> String {
        let tf = d.item(id).unwrap().text_frame().unwrap();
        d.story(tf.story).unwrap().text.clone()
    }

    #[test]
    fn a_curve_keeps_the_story_and_stays_inside_the_frame() {
        let mut s = Session::new();
        let id = frame(&mut s, "the sea keeps its own counsel");
        for kind in ["arc", "circle", "wave", "spiral"] {
            s.execute("poem.curve", &json!({"kind": kind, "bend": 80})).unwrap();
            let d = s.doc().unwrap().doc.clone();
            let it = d.item(id).unwrap();
            assert!(it.text_frame().unwrap().options.path.is_some(), "{kind} sets the text on its path");
            assert_eq!(story_text(&d, id), "the sea keeps its own counsel", "{kind} keeps the words");
            let b = it.path.bounds().unwrap();
            assert!(b.x0 >= 72.0 - 1e-6 && b.x1 <= 372.0 + 1e-6 && b.y0 >= 72.0 - 1e-6 && b.y1 <= 272.0 + 1e-6, "{kind}: {b:?}");
        }
        let up = |s: &mut Session, bend: f64| {
            s.execute("poem.curve", &json!({"kind": "arc", "bend": bend})).unwrap();
            let it = s.doc().unwrap().doc.item(id).unwrap().clone();
            let sp = &it.path.subpaths[0];
            // The middle of the arc against its ends: above them when it bows up.
            it.path.bounds().unwrap().center().y - sp.anchors[0].p.y
        };
        assert!(up(&mut s, 60.0) < 0.0 && up(&mut s, -60.0) > 0.0, "an arc bows up or down");
        s.execute("poem.curve", &json!({"kind": "none"})).unwrap();
        let d = s.doc().unwrap().doc.clone();
        assert!(d.item(id).unwrap().text_frame().unwrap().options.path.is_none());
        assert_eq!(d.item(id).unwrap().shape, Shape::Rectangle);
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(s.doc().unwrap().doc.item(id).unwrap().text_frame().unwrap().options.path.is_some(), "undo brings the curve back");
    }

    #[test]
    fn an_empty_frame_or_an_unknown_curve_is_refused() {
        let mut s = Session::new();
        let _ = frame(&mut s, "x");
        let r = s.execute("frame.create", &json!({"rect": [0, 0, 50, 50]})).unwrap();
        assert!(s.execute("poem.curve", &json!({"kind": "arc", "ids": [r["id"]]})).is_err());
        assert!(s.execute("poem.curve", &json!({"kind": "zigzag"})).is_err());
    }

    #[test]
    fn splitting_keeps_every_word_its_formatting_and_its_place() {
        let mut s = Session::new();
        let id = frame(&mut s, "the sea keeps\nits own counsel");
        let tf_story = s.doc().unwrap().doc.item(id).unwrap().text_frame().unwrap().story;
        // "sea" set larger.
        s.execute("text.select", &json!({"story": tf_story.0, "anchor": 4, "focus": 7})).unwrap();
        s.execute("type.char", &json!({"attrs": {"size": 24}})).unwrap();
        let before = s.doc().unwrap().doc.clone();
        s.execute("selection.set", &json!({"ids": [id.0]})).unwrap();
        let ids: Vec<ItemId> = s.execute("poem.split", &json!({"by": "words"})).unwrap()["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| ItemId(v.as_u64().unwrap()))
            .collect();
        let d = s.doc().unwrap().doc.clone();
        assert!(d.item(id).is_none() && d.story(tf_story).is_none());
        let words: Vec<String> = ids.iter().map(|i| story_text(&d, *i)).collect();
        assert_eq!(words, ["the", "sea", "keeps", "its", "own", "counsel"]);
        let sea = d.story(d.item(ids[1]).unwrap().text_frame().unwrap().story).unwrap();
        let was = before.story(tf_story).unwrap();
        assert_eq!(sea.char_format_at(1), was.char_format_at(5), "a word keeps its formatting");
        // Words on one line sit left to right; the second line sits below the first.
        let b: Vec<Rect> = ids.iter().map(|i| d.item(*i).unwrap().bounds()).collect();
        assert!(b[0].x1 <= b[1].x0 + 2.5 && b[1].x1 <= b[2].x0 + 2.5, "{b:?}");
        assert!(b[3].y0 > b[0].y0 + 1.0);
        assert!(b.iter().all(|r| r.x0 >= 70.0 && r.y0 >= 70.0 && r.x1 <= 374.0), "{b:?}");
        // Nothing is overset.
        for i in &ids {
            let sid = d.item(*i).unwrap().text_frame().unwrap().story;
            let cs = designcraft_compose::compose_story(&d, sid, &Default::default());
            assert!(cs.overset_at.is_none(), "{:?} is overset", story_text(&d, *i));
        }
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(story_text(&s.doc().unwrap().doc, id), "the sea keeps\nits own counsel");
    }

    #[test]
    fn splitting_by_lines_keeps_each_line_whole() {
        let mut s = Session::new();
        let _ = frame(&mut s, "the sea keeps\nits  own  counsel");
        let r = s.execute("poem.split", &json!({"by": "lines"})).unwrap();
        let d = s.doc().unwrap().doc.clone();
        let lines: Vec<String> = r["ids"].as_array().unwrap().iter().map(|v| story_text(&d, ItemId(v.as_u64().unwrap()))).collect();
        assert_eq!(lines, ["the sea keeps", "its  own  counsel"]);
    }
}
