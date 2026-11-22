//! Poem panel: shape a poem's text frame with clicks. Each button runs a registered command:
//! Curve Text (`poem.curve`), Convert Shape, Rotate and Flip, Story Direction and Split Text
//! Frame (`poem.split`).

use serde_json::{Value, json};

use crate::DesignApp;
use crate::theme::Tokens;

pub fn show(app: &mut DesignApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let lang = app.ui.language.clone();
    let tr = |s: &str| crate::i18n::tr(&lang, s).to_string();
    let text_frame = app.session.active().is_some_and(|d| {
        !d.selection.items.is_empty() && d.selection.items.iter().all(|id| d.doc.item(*id).is_some_and(|it| it.text_frame().is_some()))
    });
    let one = app.session.active().is_some_and(|d| d.selection.items.len() == 1);
    if !text_frame {
        ui.label(crate::rtl::widget(ui, egui::RichText::new(tr("Select a text frame.")).size(11.0).color(t.text_dim)));
    }
    let bend_id = egui::Id::unique("poem_bend");
    let mut bend: f64 = ui.data(|d| d.get_temp(bend_id)).unwrap_or(50.0);
    let run = |app: &mut DesignApp, id: &str, p: Value| {
        if let Err(e) = app.run(id, p) {
            app.status_error("Poem", e);
        }
    };
    ui.add_enabled_ui(text_frame, |ui| {
        heading(ui, &tr("Curve Text"));
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [("none", "None"), ("arc", "Arc"), ("circle", "Circle"), ("wave", "Wave"), ("spiral", "Spiral")] {
                if ui.button(crate::rtl::widget(ui, tr(label))).clicked() {
                    run(app, "poem.curve", json!({"kind": kind, "bend": bend}));
                }
            }
        });
        ui.horizontal(|ui| {
            crate::rtl::label(ui, tr("Bend:"));
            ui.add(egui::Slider::new(&mut bend, -100.0..=100.0).step_by(5.0).show_value(false)).accessible_name(tr("Bend"));
            ui.label(format!("{bend:.0}%"));
        });
        ui.separator();
        heading(ui, &tr("Convert Shape"));
        ui.horizontal_wrapped(|ui| {
            for (to, label) in [("rectangle", "Rectangle"), ("ellipse", "Ellipse"), ("triangle", "Triangle"), ("polygon", "Polygon")] {
                if ui.button(crate::rtl::widget(ui, tr(label))).clicked() {
                    run(app, "object.convertShape", json!({"to": to}));
                }
            }
        });
        ui.separator();
        heading(ui, &tr("Transform"));
        ui.horizontal_wrapped(|ui| {
            for (angle, label) in [(-90.0, "Rotate 90° Clockwise"), (90.0, "Rotate 90° Anticlockwise"), (180.0, "Rotate 180°")] {
                if ui.button(crate::rtl::widget(ui, tr(label))).clicked() {
                    run(app, "transform.rotate", json!({"angle": angle}));
                }
            }
            for (axis, label) in [("horizontal", "Flip Horizontal"), ("vertical", "Flip Vertical")] {
                if ui.button(crate::rtl::widget(ui, tr(label))).clicked() {
                    run(app, "transform.flip", json!({"axis": axis}));
                }
            }
        });
        ui.separator();
        heading(ui, &tr("Story Direction"));
        ui.horizontal_wrapped(|ui| {
            for (vertical, label) in [(false, "Horizontal"), (true, "Vertical")] {
                if ui.button(crate::rtl::widget(ui, tr(label))).clicked() {
                    run(app, "type.storyDirection", json!({"vertical": vertical}));
                }
            }
        });
        ui.separator();
        heading(ui, &tr("Split Text Frame"));
        ui.add_enabled_ui(one, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (by, label) in [("words", "Split into Words"), ("lines", "Split into Lines")] {
                    if ui.button(crate::rtl::widget(ui, tr(label))).clicked() {
                        run(app, "poem.split", json!({"by": by}));
                    }
                }
            });
        });
    });
    ui.data_mut(|d| d.insert_temp(bend_id, bend));
}

fn heading(ui: &mut egui::Ui, text: &str) {
    crate::rtl::label(ui, egui::RichText::new(text).size(11.5).strong());
}

#[cfg(test)]
mod tests {
    use egui_kittest::{Harness, kittest::Queryable};
    use serde_json::json;

    use super::*;

    fn harness(app: DesignApp) -> Harness<'static, DesignApp> {
        let mut h = Harness::builder().with_size(egui::vec2(300.0, 420.0)).build_ui_state(
            |ui, app: &mut DesignApp| {
                let id = egui::Id::unique("test_fonts");
                if ui.data(|d| d.get_temp::<bool>(id)).is_none() {
                    crate::theme::install_fonts(ui.ctx(), "");
                    ui.data_mut(|d| d.insert_temp(id, true));
                    return;
                }
                show(app, ui);
            },
            app,
        );
        h.run_steps(3);
        h
    }

    #[test]
    fn the_panel_curves_and_splits_through_its_commands() {
        let mut app = DesignApp::new(designcraft_engine::Session::new(), crate::Services::default());
        app.session.execute("file.new", &json!({})).unwrap();
        let id = app.session.execute("frame.create", &json!({"rect": [72, 72, 372, 272], "content": "text", "text": "the sea keeps"})).unwrap()["id"]
            .as_u64()
            .unwrap();
        app.session.execute("selection.set", &json!({"ids": [id]})).unwrap();
        let mut h = harness(app);
        h.get_by_label("Arc").click();
        h.run_steps(2);
        let curved = |h: &Harness<'static, DesignApp>| {
            h.state()
                .session
                .active()
                .unwrap()
                .doc
                .item(designcraft_doc::ItemId(id))
                .is_some_and(|it| it.text_frame().is_some_and(|t| t.options.path.is_some()))
        };
        assert!(curved(&h));
        h.get_by_label("None").click();
        h.run_steps(2);
        assert!(!curved(&h));
        h.get_by_label("Split into Words").click();
        h.run_steps(2);
        assert_eq!(h.state().session.active().unwrap().selection.items.len(), 3);
    }
}
