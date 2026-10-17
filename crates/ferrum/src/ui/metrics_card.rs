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
use ferrum_radiomics::{
    Family, HuRange, HuRangeShares, IvhCurve, MetricsResult, Params, Profile, Value, CLINICAL, DEFAULT_HU_RANGES,
};

use super::theme::{DANGER, SURFACE, TEXT_DIM, WARN};

/// The profile the card shows.
pub const PROFILE: &Profile = &CLINICAL;

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
        "HU2" => format!("{number} HU²"),
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
        Family::Topology => "Connectivity",
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
    if let Some(volume) = view.volume {
        ui.label(RichText::new(format!("Volume: {volume}")).size(12.0).color(TEXT_DIM));
    }
    if stale {
        ui.label(
            RichText::new("Out of date: the segment, the image or the volume shown changed after this was measured.")
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
        MetricsOutcome::Ready(result) => ready(ui, viewer, label, &result),
    }
}

/// Measures segment `label` for the card.
pub fn measure(viewer: &mut Viewer, label: u8) {
    request(viewer, label);
}

fn request(viewer: &mut Viewer, label: u8) {
    if let Err(e) = viewer.request_segment_metrics(label, PROFILE) {
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

fn ready(ui: &mut egui::Ui, viewer: &mut Viewer, label: u8, result: &MetricsResult) {
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
    for w in &result.warnings {
        ui.label(RichText::new(format!("⚠ {w}")).size(12.0).color(WARN));
    }
    let all = rows(result);
    for family in [Family::Size, Family::Shape, Family::Topology, Family::Intensity] {
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
    if let Some(shares) = &result.hu_ranges {
        hu_range_table(ui, label, shares);
    }
    if let Some(curve) = &result.ivh {
        ivh_plot(ui, label, curve);
    }
    settings(ui, viewer, label);
}

/// One line of the table of HU ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct RangeRow {
    /// The range as written, for example `-100 to 0 HU`.
    pub range: String,
    /// Voxels in it.
    pub voxels: u64,
    /// Share of the segment, as a percentage with one decimal.
    pub share: String,
}

fn bound(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{x:.0}")
    } else {
        format!("{x}")
    }
}

/// The lines of the table of HU ranges: one per range and the rest.
pub fn range_rows(shares: &HuRangeShares) -> Vec<RangeRow> {
    let mut out: Vec<RangeRow> = shares
        .ranges
        .iter()
        .map(|r| RangeRow {
            range: format!("{} to {} HU", bound(r.range.lo), bound(r.range.hi)),
            voxels: r.voxels,
            share: format!("{:.1} %", 100.0 * r.fraction),
        })
        .collect();
    out.push(RangeRow {
        range: "outside these ranges".to_owned(),
        voxels: shares.outside_voxels,
        share: format!("{:.1} %", 100.0 * shares.outside),
    });
    out
}

fn hu_range_table(ui: &mut egui::Ui, label: u8, shares: &HuRangeShares) {
    egui::CollapsingHeader::new("Share of voxels by HU range")
        .id_salt(("metrics-ranges", label))
        .default_open(true)
        .show(ui, |ui| {
            let note = if shares.is_default {
                "Default ranges for orientation, not clinical thresholds. Change them under Histogram and HU ranges."
            } else {
                "Ranges as you set them. Each range includes its lower value and not its upper."
            };
            ui.label(RichText::new(note).size(11.0).color(TEXT_DIM));
            egui::Grid::new(("metrics-ranges-grid", label)).num_columns(3).show(ui, |ui| {
                for row in range_rows(shares) {
                    ui.label(RichText::new(row.range).size(12.0).color(TEXT_DIM));
                    ui.label(RichText::new(row.voxels.to_string()).size(12.0));
                    ui.label(RichText::new(row.share).size(12.0));
                    ui.end_row();
                }
            });
        });
}

/// The intensity-volume histogram: the share of the volume (up) at or above
/// each value (right).
fn ivh_plot(ui: &mut egui::Ui, label: u8, curve: &IvhCurve) {
    egui::CollapsingHeader::new("Intensity-volume histogram").id_salt(("metrics-ivh", label)).default_open(false).show(
        ui,
        |ui| {
            let (lo, hi) = (curve.intensity[0], curve.intensity[curve.intensity.len() - 1]);
            ui.label(
                RichText::new(format!(
                    "Share of the volume at or above each value, from {} to {} HU.",
                    bound(lo.round()),
                    bound(hi.round())
                ))
                .size(11.0)
                .color(TEXT_DIM),
            );
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width().clamp(120.0, 320.0), 90.0), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, SURFACE);
            let n = curve.volume_fraction.len() as f32 - 1.0;
            let points: Vec<egui::Pos2> = curve
                .volume_fraction
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    egui::pos2(rect.left() + rect.width() * i as f32 / n, rect.bottom() - rect.height() * *f as f32)
                })
                .collect();
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, ui.visuals().selection.bg_fill)));
            response.on_hover_text("Starts at 100 % and never rises. V10, V90, I10 and I90 are read from this curve.");
        },
    );
}

/// The text fields of the settings and what is wrong with them.
#[derive(Debug, Clone, Default)]
struct SettingsEdit {
    loaded: bool,
    bin_width: String,
    ranges: Vec<(String, String)>,
    error: Option<String>,
}

impl SettingsEdit {
    fn from_params(params: &Params) -> Self {
        let (ranges, _) = params.ranges();
        Self {
            loaded: true,
            bin_width: bound(params.bin_width),
            ranges: ranges.iter().map(|r| (bound(r.lo), bound(r.hi))).collect(),
            error: None,
        }
    }
}

/// Parameters from the text of the fields; ranges equal to the defaults are
/// kept as "defaults" so the result says so.
pub fn parse_params(bin_width: &str, ranges: &[(String, String)]) -> Result<Params, String> {
    let number = |what: &str, text: &str| {
        text.trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| format!("{what}: {text:?} is not a number"))
    };
    let mut parsed = Vec::new();
    for (lo, hi) in ranges.iter().filter(|(lo, hi)| !(lo.trim().is_empty() && hi.trim().is_empty())) {
        parsed.push(HuRange::new(number("range from", lo)?, number("range to", hi)?));
    }
    let params = Params {
        bin_width: number("bin width", bin_width)?,
        hu_ranges: (parsed != DEFAULT_HU_RANGES).then_some(parsed),
        ..Params::default()
    };
    params.validate().map_err(|e| e.to_string())?;
    Ok(params)
}

fn settings(ui: &mut egui::Ui, viewer: &mut Viewer, label: u8) {
    let id = egui::Id::new(("metrics-settings", label));
    let mut edit: SettingsEdit = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    if !edit.loaded {
        edit = SettingsEdit::from_params(viewer.metrics_params());
    }
    let mut apply = false;
    egui::CollapsingHeader::new("Histogram and HU ranges").id_salt(("metrics-settings-header", label)).default_open(false).show(
        ui,
        |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Bin width (HU)").size(12.0).color(TEXT_DIM))
                    .on_hover_text("Width of a histogram bin for entropy and uniformity. Bins are [k × width, (k + 1) × width).");
                let field = ui.add(egui::TextEdit::singleline(&mut edit.bin_width).desired_width(50.0));
                field.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Bin width field"));
            });
            ui.label(RichText::new("HU ranges (from, to)").size(12.0).color(TEXT_DIM))
                .on_hover_text("Each range includes its lower value and not its upper. Ranges must not overlap; leave a row empty to drop it.");
            let mut remove = None;
            for (i, (lo, hi)) in edit.ranges.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(lo).desired_width(50.0));
                    ui.label("to");
                    ui.add(egui::TextEdit::singleline(hi).desired_width(50.0));
                    if ui.small_button("Remove").on_hover_text("Drop this range").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                edit.ranges.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.button("Add range").clicked() {
                    edit.ranges.push((String::new(), String::new()));
                }
                if ui.button("Use defaults").clicked() {
                    edit = SettingsEdit::from_params(&Params::default());
                }
            });
            if let Some(e) = &edit.error {
                ui.label(RichText::new(e).size(12.0).color(DANGER));
            }
            apply = ui.button("Apply and measure again").clicked();
        },
    );
    if apply {
        match parse_params(&edit.bin_width, &edit.ranges).and_then(|p| viewer.set_metrics_params(p)) {
            Ok(()) => {
                edit.error = None;
                request(viewer, label);
            }
            Err(e) => edit.error = Some(e),
        }
    }
    ui.data_mut(|d| d.insert_temp(id, edit));
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
        assert_eq!(shape.len(), 25);
        let area = shape.iter().find(|l| l.label == "Surface area").unwrap();
        assert!(area.value.ends_with(" mm²"), "{}", area.value);
        // a cube has no unique minor or least axis: the dash, and the reason on hover
        let tie = shape.iter().find(|l| l.label.starts_with("Minor axis direction x")).unwrap();
        assert_eq!(tie.value, "—");
        assert!(tie.reason.as_deref().unwrap_or("").contains("not unique"));
        assert!(shape.iter().find(|l| l.label == "Sphericity").unwrap().value.parse::<f64>().is_ok());
        // a cube is its own hull: solidity 1; the connectivity group says one piece, no cavity
        assert_eq!(shape.iter().find(|l| l.label == "Solidity").unwrap().value, "1.00");
        let links: Vec<(&str, &str)> =
            lines.iter().filter(|l| l.family == Family::Topology).map(|l| (l.label, l.value.as_str())).collect();
        assert_eq!(
            links,
            [
                ("Pieces", "1"),
                ("Largest piece share", "1"),
                ("Cavities", "0"),
                ("Cavity volume (mm³)", "0 mm³"),
                ("Euler number", "1"),
                ("Image sides touched", "3"),
            ]
        );
    }

    // covers 18.11-c
    #[test]
    fn the_fields_become_parameters_and_nonsense_is_refused_with_the_reason() {
        let fields = |r: &[(&str, &str)]| r.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        let defaults: Vec<(String, String)> = DEFAULT_HU_RANGES.iter().map(|r| (bound(r.lo), bound(r.hi))).collect();
        // the default ranges stay "default" so the result says so
        let p = parse_params("25", &defaults).unwrap();
        assert!(p.hu_ranges.is_none() && p.bin_width == 25.0);
        let p = parse_params(" 12,5 ", &fields(&[("-50", "50"), ("50", "150.5"), ("", "")])).unwrap();
        assert_eq!(p.bin_width, 12.5, "a decimal comma is read");
        assert_eq!(
            p.hu_ranges,
            Some(vec![HuRange::new(-50.0, 50.0), HuRange::new(50.0, 150.5)]),
            "empty rows are dropped"
        );
        for (bin, ranges, expect) in [
            ("abc", vec![], "bin width"),
            ("0", vec![], "bin_width"),
            ("25", vec![("0", "x")], "range to"),
            ("25", vec![("0", "100"), ("50", "150")], "overlap"),
            ("25", vec![("100", "0")], "min below max"),
            ("25", vec![("5", "")], "range to"),
        ] {
            let e = parse_params(bin, &fields(&ranges)).unwrap_err();
            assert!(e.contains(expect), "{bin} {ranges:?}: {e}");
        }
    }

    // covers 18.11-a, 18.12-a
    #[test]
    fn the_range_table_has_a_line_per_range_and_one_for_the_rest() {
        use ferrum_domain::{Dims3, MaskRegion, Volume};
        use ferrum_radiomics::{compute, Input, ValueUnit};
        let dims = Dims3::new(8, 8, 8);
        let values: Vec<f32> = (0..dims.voxel_count()).map(|n| if n % 64 < 32 { -50.0 } else { 50.0 }).collect();
        let volume = Volume::from_physical(dims, glam::Vec3::ONE, &values).unwrap();
        let region = MaskRegion::from_grid(dims, |n| n % 8 < 4 && (n / 8) % 8 < 4 && n / 64 < 4).unwrap();
        let params =
            Params { hu_ranges: Some(vec![HuRange::new(-100.0, 0.0), HuRange::new(0.0, 25.0)]), ..Params::default() };
        let result =
            compute(&Input { region: &region, volume: &volume, unit: ValueUnit::Hu }, &CLINICAL, &params).unwrap();
        let rows = range_rows(result.hu_ranges.as_ref().unwrap());
        // 64 voxels: the 4 x 4 x 4 corner; rows n % 64 < 32 are -50 (j < 4), so all 64 are -50
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].range.as_str(), rows[0].voxels, rows[0].share.as_str()), ("-100 to 0 HU", 64, "100.0 %"));
        assert_eq!((rows[1].range.as_str(), rows[1].voxels, rows[1].share.as_str()), ("0 to 25 HU", 0, "0.0 %"));
        assert_eq!(
            (rows[2].range.as_str(), rows[2].voxels, rows[2].share.as_str()),
            ("outside these ranges", 0, "0.0 %")
        );
        // the rest counts the voxels in no range
        let params = Params { hu_ranges: Some(vec![HuRange::new(0.0, 25.0)]), ..Params::default() };
        let result =
            compute(&Input { region: &region, volume: &volume, unit: ValueUnit::Hu }, &CLINICAL, &params).unwrap();
        let rows = range_rows(result.hu_ranges.as_ref().unwrap());
        assert_eq!((rows[1].voxels, rows[1].share.as_str()), (64, "100.0 %"));
    }

    #[test]
    fn values_are_formatted_for_people() {
        assert_eq!(format_value(&Value::Number(64.0), ""), "64");
        assert_eq!(format_value(&Value::Number(0.128), "mL"), "0.128 mL");
        assert_eq!(format_value(&Value::Number(432.0), "mm3"), "432 mm³");
        assert_eq!(format_value(&Value::Number(326.006), "mm2"), "326.01 mm²");
        assert_eq!(format_value(&Value::Number(-50.0), "HU"), "-50 HU");
        assert_eq!(format_value(&Value::Number(960_000.0), "HU2"), "960000 HU²");
        assert_eq!(format_value(&Value::Number(2.0), "bits"), "2 bits");
        assert_eq!(format_value(&Value::Number(111.80339), "HU"), "111.80 HU");
        assert_eq!(format_value(&Value::Null { reason: "r".into() }, "mm"), "—");
    }
}
