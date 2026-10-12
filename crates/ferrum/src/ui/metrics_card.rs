//! The metrics card of the Segments panel and the dialog that asks what
//! kind of image was opened (Stage 18, `docs/radiomics.md`).
//!
//! The card shows the features of `ferrum-radiomics` with their units; the
//! definition of each is its hover text. It says whether a segment was
//! confirmed by a person, whether the numbers are out of date, and why
//! there are none when the image type is not known.

use egui::RichText;
use ferrum_app::{MetricsOutcome, Viewer};
use ferrum_domain::ReviewStatus;
use ferrum_radiomics::{Family, MetricsResult, Value, QUICK};

use super::theme::{DANGER, SURFACE, TEXT_DIM, WARN};

/// One line of the card.
#[derive(Debug, Clone, PartialEq)]
pub struct CardRow {
    /// Group of the feature.
    pub family: Family,
    /// Name for people.
    pub label: &'static str,
    /// Value with its unit, or a dash if there is none.
    pub value: String,
    /// What it is and how it is computed (the hover text).
    pub definition: &'static str,
    /// Why there is no value, if there is none.
    pub reason: Option<String>,
}

/// A value as the card shows it: whole numbers without decimals, millilitres
/// with three, anything else with two.
pub fn format_value(value: &Value, unit: &str) -> String {
    let Value::Number(x) = value else { return "—".to_owned() };
    let number = if unit == "mL" {
        format!("{x:.3}")
    } else if x.fract() == 0.0 && x.abs() < 1e12 {
        format!("{x:.0}")
    } else {
        format!("{x:.2}")
    };
    match unit {
        "" => number,
        "mm3" => format!("{number} mm³"),
        "mm2" => format!("{number} mm²"),
        u => format!("{number} {u}"),
    }
}

/// The lines of the card for `result`, in the order of the profile.
pub fn rows(result: &MetricsResult) -> Vec<CardRow> {
    result
        .features
        .iter()
        .map(|f| CardRow {
            family: f.feature.family,
            label: f.feature.label,
            value: format_value(&f.value, &f.unit),
            definition: f.feature.definition,
            reason: match &f.value {
                Value::Null { reason } => Some(reason.clone()),
                Value::Number(_) => None,
            },
        })
        .collect()
}

fn family_title(family: Family) -> &'static str {
    match family {
        Family::Size => "Size",
        Family::Shape => "Shape",
        Family::Intensity => "Intensity",
    }
}

/// The note under the title of the card for segments nobody confirmed.
fn status_note(status: ReviewStatus) -> Option<&'static str> {
    match status {
        ReviewStatus::Confirmed => None,
        ReviewStatus::Proposed => Some("proposed · not confirmed by a person"),
        ReviewStatus::Rejected => Some("rejected"),
    }
}

/// Draws the card of segment `label`.
pub fn card(ui: &mut egui::Ui, viewer: &mut Viewer, label: u8, name: &str, status: ReviewStatus) {
    ui.add_space(4.0);
    let frame = egui::Frame::new().fill(SURFACE).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 6));
    frame.show(ui, |ui| {
        ui.set_width((ui.available_width() - 16.0).max(140.0));
        card_body(ui, viewer, label, name, status);
    });
}

fn card_body(ui: &mut egui::Ui, viewer: &mut Viewer, label: u8, name: &str, status: ReviewStatus) {
    ui.label(RichText::new(format!("Metrics · {name}")).strong());
    if let Some(note) = status_note(status) {
        ui.label(RichText::new(note).size(12.0).color(WARN));
    }
    let Some(view) = viewer.segment_metrics_view(label) else {
        if ui.button("Measure").clicked() {
            request(viewer, label);
        }
        return;
    };
    let stale = view.stale;
    let outcome = view.outcome.clone();
    if stale {
        ui.label(
            RichText::new("Out of date: the segment or the image changed after this was measured.")
                .size(12.0)
                .color(WARN),
        );
        if ui.button("Measure again").clicked() {
            request(viewer, label);
        }
    }
    match outcome {
        MetricsOutcome::Pending => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Measuring…").color(TEXT_DIM));
            });
        }
        MetricsOutcome::Failed(why) => failed(ui, viewer, label, &why),
        MetricsOutcome::Ready(result) => ready(ui, label, &result),
    }
}

fn request(viewer: &mut Viewer, label: u8) {
    if let Err(e) = viewer.request_segment_metrics(label, &QUICK) {
        viewer.status.errors.push(e);
    }
}

fn failed(ui: &mut egui::Ui, viewer: &mut Viewer, label: u8, why: &str) {
    ui.label(RichText::new(why).size(12.0).color(DANGER));
    if viewer.can_declare_modality() && ui.button("Set image type…").clicked() {
        viewer.ask_modality_again();
    }
    if ui.button("Measure again").clicked() {
        request(viewer, label);
    }
}

fn ready(ui: &mut egui::Ui, label: u8, result: &MetricsResult) {
    ui.label(
        RichText::new(format!(
            "Profile {} {} · values in {}",
            result.profile,
            result.profile_version,
            if result.unit.label().is_empty() { "image units" } else { result.unit.label() }
        ))
        .size(11.0)
        .color(TEXT_DIM),
    );
    let all = rows(result);
    for family in [Family::Size, Family::Shape, Family::Intensity] {
        let lines: Vec<&CardRow> = all.iter().filter(|r| r.family == family).collect();
        if lines.is_empty() {
            continue;
        }
        egui::CollapsingHeader::new(family_title(family))
            .id_salt(("metrics", label, family_title(family)))
            .default_open(true)
            .show(ui, |ui| {
                egui::Grid::new(("metrics-grid", label, family_title(family))).num_columns(2).show(ui, |ui| {
                    for r in lines {
                        let name = ui.label(RichText::new(r.label).size(12.0).color(TEXT_DIM));
                        name.on_hover_text(r.definition);
                        let value = ui.label(RichText::new(&r.value).size(12.0));
                        if let Some(reason) = &r.reason {
                            value.on_hover_text(format!("No value: {reason}"));
                        } else {
                            value.on_hover_text(r.definition);
                        }
                        ui.end_row();
                    }
                });
            });
    }
}

/// UI-only state of the image-type dialog.
#[derive(Debug, Clone, Default)]
pub struct ModalityDialog {
    other: String,
    error: Option<String>,
}

/// Asks what kind of image was opened, while the viewer needs to know.
pub fn modality_dialog(ctx: &egui::Context, viewer: &mut Viewer, state: &mut ModalityDialog) {
    if !viewer.needs_modality() {
        return;
    }
    let mut chosen: Option<String> = None;
    let mut skip = false;
    egui::Window::new("Image type")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.label("This file does not say what kind of image it is. Voxel values are read as HU only for CT.");
            ui.horizontal(|ui| {
                for (code, text) in [("CT", "CT (HU)"), ("MR", "MR"), ("PT", "PET")] {
                    if ui.button(text).clicked() {
                        chosen = Some(code.to_owned());
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Other:");
                ui.add(egui::TextEdit::singleline(&mut state.other).desired_width(80.0).hint_text("e.g. US"));
                if ui.add_enabled(!state.other.trim().is_empty(), egui::Button::new("Set")).clicked() {
                    chosen = Some(state.other.clone());
                }
            });
            if let Some(e) = &state.error {
                ui.label(RichText::new(e).size(12.0).color(DANGER));
            }
            if ui.button("Not now").clicked() {
                skip = true;
            }
        });
    if let Some(code) = chosen {
        match viewer.declare_modality(&code) {
            Ok(_) => *state = ModalityDialog::default(),
            Err(e) => state.error = Some(e),
        }
    } else if skip {
        viewer.skip_modality_prompt();
        *state = ModalityDialog::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 18.4-e
    #[test]
    fn every_row_carries_the_definition_of_its_feature() {
        use ferrum_domain::{Dims3, MaskRegion, Volume};
        use ferrum_radiomics::{compute, Input, Params, ValueUnit, QUICK};
        let dims = Dims3::new(6, 6, 6);
        let volume = Volume::from_physical(dims, glam::Vec3::ONE, &vec![10.0; dims.voxel_count()]).unwrap();
        let region = MaskRegion::from_grid(dims, |n| n % 6 < 3).unwrap();
        let result =
            compute(&Input { region: &region, volume: &volume, unit: ValueUnit::Hu }, &QUICK, &Params::default())
                .unwrap();
        let lines = rows(&result);
        assert_eq!(lines.len(), QUICK.features.len());
        for (line, id) in lines.iter().zip(QUICK.features) {
            let feature = ferrum_radiomics::feature_by_id(id).unwrap();
            assert_eq!((line.label, line.definition), (feature.label, feature.definition), "{id}");
            assert!(line.definition.len() > 20 && line.reason.is_none(), "{id}");
        }
        let volume_line = lines.iter().find(|l| l.label == "Volume").unwrap();
        assert_eq!(volume_line.value, "0.108 mL", "3 x 6 x 6 voxels of 1 mm³");
        let mean = lines.iter().find(|l| l.label == "Mean").unwrap();
        assert_eq!(mean.value, "10 HU");
    }

    #[test]
    fn the_rows_of_the_clinical_profile_have_a_shape_group_with_units() {
        use ferrum_domain::{Dims3, MaskRegion, Volume};
        use ferrum_radiomics::{compute, Input, Params, ValueUnit, CLINICAL};
        let dims = Dims3::new(8, 8, 8);
        let volume = Volume::from_physical(dims, glam::Vec3::ONE, &vec![10.0; dims.voxel_count()]).unwrap();
        let region = MaskRegion::from_grid(dims, |n| n % 8 < 4 && (n / 8) % 8 < 4 && n / 64 < 4).unwrap();
        let result =
            compute(&Input { region: &region, volume: &volume, unit: ValueUnit::Hu }, &CLINICAL, &Params::default())
                .unwrap();
        let lines = rows(&result);
        let shape: Vec<&CardRow> = lines.iter().filter(|l| l.family == Family::Shape).collect();
        assert_eq!(shape.len(), 21);
        let area = shape.iter().find(|l| l.label == "Surface area").unwrap();
        assert!(area.value.ends_with(" mm²"), "{}", area.value);
        // a cube has no unique minor or least axis: the dash, and the reason on hover
        let tie = shape.iter().find(|l| l.label.starts_with("Minor axis direction x")).unwrap();
        assert_eq!(tie.value, "—");
        assert!(tie.reason.as_deref().unwrap_or("").contains("not unique"));
        assert!(shape.iter().find(|l| l.label == "Sphericity").unwrap().value.parse::<f64>().is_ok());
    }

    #[test]
    fn values_are_formatted_for_people() {
        assert_eq!(format_value(&Value::Number(64.0), ""), "64");
        assert_eq!(format_value(&Value::Number(0.128), "mL"), "0.128 mL");
        assert_eq!(format_value(&Value::Number(432.0), "mm3"), "432 mm³");
        assert_eq!(format_value(&Value::Number(326.006), "mm2"), "326.01 mm²");
        assert_eq!(format_value(&Value::Number(-50.0), "HU"), "-50 HU");
        assert_eq!(format_value(&Value::Number(111.80339), "HU"), "111.80 HU");
        assert_eq!(format_value(&Value::Null { reason: "r".into() }, "mm"), "—");
    }
}
