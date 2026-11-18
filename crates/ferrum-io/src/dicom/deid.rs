//! DICOM de-identification: the PS3.15 Annex E **Basic Application Level
//! Confidentiality Profile** and its options (Stage 31, #75; ADR 0020).
//!
//! The attribute rules are **data, not code**: [`deidentify`] reads them from
//! the committed [`deid_table.csv`](include_str!("deid_table.csv")) — one base
//! row per Table E.1-1 attribute plus one row per governing option
//! (`tag,keyword,action,option`). The table was transcribed once from the
//! standard (DICOM PS3.15 2026d, Annex E, Table E.1-1, fetched 2026-11-17 from
//! <https://dicom.nema.org/medical/dicom/current/output/chtml/part15/chapter_E.html>),
//! with keywords and VRs cross-checked against `dicom-dictionary-std` and the
//! pydicom/deid Basic Profile recipe. A profile correction is a data edit; the
//! agreement test (`tests/deid.rs`) fails when the table and the generated
//! rules disagree.
//!
//! Actions (Table E.1-1a): `D` replace with a non-empty consistent dummy, `Z`
//! zero-length (a dummy is permitted), `X` remove, `U` consistent pseudonymous
//! UID, `C` clean, `K` keep. Compound codes (`X/Z`, `X/D`, `X/Z/D`, `Z/D`,
//! `X/Z/U*`) are IOD-conformance dependent; without an IOD-type model FERRUM
//! takes the standard's primary, most conservative branch (`X`, or `Z` for
//! `Z/D`). Two `Z` rows receive a documented non-empty dummy instead of
//! zero-length: `PatientName` → `ANONYMOUS`, `PatientID` → a salted per-patient
//! pseudonym (the `Z`/`Z/D` dummy clause), so instances stay linkable in a run.
//!
//! Options (each changes exactly the rows the table marks option-dependent):
//! `retain_uids`, `retain_device_identity`, `retain_institution_identity`,
//! `retain_patient_characteristics`, `retain_dates_modified`,
//! `clean_descriptors`. The default is the Basic Profile with no options (the
//! most conservative); an unknown option name is refused.
//!
//! Conformance choices (described here per E.3.5/E.3.6, which require the
//! cleaning/date-modification manner to be documented):
//! - `retain_dates_modified`: `DA` dates shift by a consistent per-patient
//!   offset (days); `TM` times are kept; `DT` shifts the date part and keeps the
//!   time part (preserves fine temporal relationships, E.3.6). Adds
//!   `LongitudinalTemporalInformationModified = MODIFIED`.
//! - `clean_descriptors`: free text is scrubbed of the object's own identifier
//!   values (name parts, IDs, accession, dates) and of long digit runs; this is
//!   a conservative pattern/known-identifier scrub, not a linguistic analyser.
//! - UIDs reuse the export's salted derivation ([`super::export::uid_from`])
//!   through [`UidMap`], consistent within a run — one derivation, not a second.
//! - Private (odd-group) attributes, curve groups (`50xx`), overlay comments/data
//!   (`60xx,4000`/`60xx,3000`) and group `0004` are removed (Table E.1-1 wildcard
//!   rows and E.1.1). Sequences are processed recursively so nested table
//!   attributes are protected too; pixel data and non-table attributes are kept.
//!
//! File Meta Information replacement (E.1.1 item 7) is the file writer's
//! responsibility: [`deidentify_file`] de-identifies the data set and then
//! replaces the File Meta Information consistently (the export writer and the
//! oracle harness both go through it). [`deidentify`] alone operates on the
//! data set only.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use dicom_core::header::Header;
use dicom_core::value::DataSetSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{FileDicomObject, InMemDicomObject};
use sha2::{Digest, Sha256};

use super::export::uid_from;
use super::header::str_of;

type Elem = DataElement<InMemDicomObject>;

/// Maximum sequence nesting processed; deeper sequences are emptied so a
/// crafted file cannot drive unbounded recursion.
const MAX_DEPTH: usize = 32;

/// A de-identification action code (PS3.15 Table E.1-1a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Replace with a non-empty consistent dummy value.
    D,
    /// Replace with a zero-length value (a dummy is permitted).
    Z,
    /// Remove the attribute.
    X,
    /// Replace with a consistent pseudonymous UID.
    U,
    /// Clean (replace with non-identifying values of similar meaning).
    C,
    /// Keep unchanged.
    K,
    /// `X/Z`: remove unless zero-length is required for IOD conformance.
    XZ,
    /// `X/D`: remove unless a dummy is required for IOD conformance.
    XD,
    /// `X/Z/D`: remove unless zero-length or a dummy is required.
    XZD,
    /// `Z/D`: zero-length unless a dummy is required for IOD conformance.
    ZD,
    /// `X/Z/U*`: remove unless zero-length or contained-UID replacement is required.
    XZU,
}

impl Action {
    /// The standard's spelling of the action code.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::D => "D",
            Action::Z => "Z",
            Action::X => "X",
            Action::U => "U",
            Action::C => "C",
            Action::K => "K",
            Action::XZ => "X/Z",
            Action::XD => "X/D",
            Action::XZD => "X/Z/D",
            Action::ZD => "Z/D",
            Action::XZU => "X/Z/U*",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "D" => Action::D,
            "Z" => Action::Z,
            "X" => Action::X,
            "U" => Action::U,
            "C" => Action::C,
            "K" => Action::K,
            "X/Z" => Action::XZ,
            "X/D" => Action::XD,
            "X/Z/D" => Action::XZD,
            "Z/D" => Action::ZD,
            "X/Z/U*" => Action::XZU,
            _ => return None,
        })
    }

    /// The effective action after taking the primary branch of a compound code
    /// (no IOD-type model): the six actions the engine applies.
    fn effective(self) -> Effective {
        match self {
            Action::D => Effective::D,
            Action::Z | Action::ZD => Effective::Z,
            Action::X | Action::XZ | Action::XD | Action::XZD | Action::XZU => Effective::X,
            Action::U => Effective::U,
            Action::C => Effective::C,
            Action::K => Effective::K,
        }
    }
}

/// A Basic Profile option (PS3.15 E.3); each governs the table rows marked with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Opt {
    /// Retain UIDs Option (E.3.9).
    Uids,
    /// Retain Device Identity Option (E.3.8).
    Device,
    /// Retain Institution Identity Option (E.3.11).
    Institution,
    /// Retain Patient Characteristics Option (E.3.7).
    Characteristics,
    /// Retain Longitudinal Temporal Information with Modified Dates Option (E.3.6).
    DatesModified,
    /// Clean Descriptors Option (E.3.5).
    Descriptors,
}

impl Opt {
    /// The option's FERRUM name (as used in the table's `option` column).
    pub fn as_str(self) -> &'static str {
        match self {
            Opt::Uids => "retain_uids",
            Opt::Device => "retain_device_identity",
            Opt::Institution => "retain_institution_identity",
            Opt::Characteristics => "retain_patient_characteristics",
            Opt::DatesModified => "retain_dates_modified",
            Opt::Descriptors => "clean_descriptors",
        }
    }

    /// Parses an option name; `None` for an unknown name (which is refused).
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "retain_uids" => Opt::Uids,
            "retain_device_identity" => Opt::Device,
            "retain_institution_identity" => Opt::Institution,
            "retain_patient_characteristics" => Opt::Characteristics,
            "retain_dates_modified" => Opt::DatesModified,
            "clean_descriptors" => Opt::Descriptors,
            _ => return None,
        })
    }

    /// CID 7050 code `(value, meaning)` of the option.
    fn code(self) -> (&'static str, &'static str) {
        match self {
            Opt::Uids => ("113110", "Retain UIDs Option"),
            Opt::Device => ("113109", "Retain Device Identity Option"),
            Opt::Institution => ("113112", "Retain Institution Identity Option"),
            Opt::Characteristics => ("113108", "Retain Patient Characteristics Option"),
            Opt::DatesModified => ("113107", "Retain Longitudinal Temporal Information Modified Dates Option"),
            Opt::Descriptors => ("113105", "Clean Descriptors Option"),
        }
    }
}

/// The options selected on top of the Basic Profile for one run.
///
/// The default is no options (the most conservative). `uid_salt` seeds the
/// replacement UIDs and the per-patient pseudonym/date offset; set it to a
/// secret value for a real export.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileOptions {
    /// Shift dates by a consistent per-patient offset, keeping times (E.3.6).
    pub retain_dates_modified: bool,
    /// Keep the original UIDs (E.3.9).
    pub retain_uids: bool,
    /// Scrub identifiers from free-text descriptors (E.3.5).
    pub clean_descriptors: bool,
    /// Keep device identity attributes (E.3.8).
    pub retain_device_identity: bool,
    /// Keep institution identity attributes (E.3.11).
    pub retain_institution_identity: bool,
    /// Keep patient characteristics (age, sex, size, weight, …) (E.3.7).
    pub retain_patient_characteristics: bool,
    /// Salt of the replacement UIDs and per-patient pseudonyms.
    pub uid_salt: String,
}

impl ProfileOptions {
    /// Enables an option by name; refuses an unknown name (criterion 31.2-b).
    pub fn enable(&mut self, name: &str) -> Result<(), DeidError> {
        let opt = Opt::from_name(name).ok_or_else(|| DeidError::UnknownOption(name.to_owned()))?;
        self.set(opt, true);
        Ok(())
    }

    fn enabled(&self, opt: Opt) -> bool {
        match opt {
            Opt::Uids => self.retain_uids,
            Opt::Device => self.retain_device_identity,
            Opt::Institution => self.retain_institution_identity,
            Opt::Characteristics => self.retain_patient_characteristics,
            Opt::DatesModified => self.retain_dates_modified,
            Opt::Descriptors => self.clean_descriptors,
        }
    }

    fn set(&mut self, opt: Opt, value: bool) {
        match opt {
            Opt::Uids => self.retain_uids = value,
            Opt::Device => self.retain_device_identity = value,
            Opt::Institution => self.retain_institution_identity = value,
            Opt::Characteristics => self.retain_patient_characteristics = value,
            Opt::DatesModified => self.retain_dates_modified = value,
            Opt::Descriptors => self.clean_descriptors = value,
        }
    }

    /// The enabled options, in table order.
    fn enabled_opts(&self) -> Vec<Opt> {
        [Opt::Uids, Opt::Device, Opt::Institution, Opt::Characteristics, Opt::DatesModified, Opt::Descriptors]
            .into_iter()
            .filter(|o| self.enabled(*o))
            .collect()
    }
}

/// Error selecting a de-identification option.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeidError {
    /// The option name is not one of the Basic Profile options.
    #[error("unknown de-identification option: {0}")]
    UnknownOption(String),
}

/// The de-identification profile: the PS3.15 Basic Application Level
/// Confidentiality Profile with the options selected for the run.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    /// Options applied on top of the Basic Profile (none by default).
    pub options: ProfileOptions,
}

/// The per-attribute rule generated from one Table E.1-1 attribute: its Basic
/// Profile action and the option overrides (`option → action`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// The Basic Profile action (no option).
    pub base: Action,
    /// Option overrides, in table order; an enabled option replaces the base.
    pub overrides: Vec<(Opt, Action)>,
}

/// The generated rules, keyed by tag (wildcard/group rows are handled separately).
pub fn rules() -> &'static BTreeMap<Tag, Rule> {
    static RULES: OnceLock<BTreeMap<Tag, Rule>> = OnceLock::new();
    RULES.get_or_init(|| parse_table(include_str!("deid_table.csv")))
}

/// Parses the committed table into per-tag rules. Wildcard rows (`50xx`, `60xx`,
/// `gggg`) are group rules handled by [`removed_by_group_rule`], not single tags.
/// The tag field `(gggg,eeee)` itself contains a comma, so each line is split
/// after the tag's closing parenthesis before the remaining fields are split.
fn parse_table(csv: &str) -> BTreeMap<Tag, Rule> {
    let mut map: BTreeMap<Tag, Rule> = BTreeMap::new();
    for line in csv.lines().skip(1) {
        let Some((tag_part, rest)) = line.split_once(')') else { continue };
        let tag_s = format!("{tag_part})");
        let rest = rest.strip_prefix(',').unwrap_or(rest);
        let mut fields = rest.split(',');
        let (Some(_keyword), Some(action_s), Some(option_s)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let Some(tag) = parse_tag(&tag_s) else { continue };
        let Some(action) = Action::parse(action_s) else { continue };
        let rule = map.entry(tag).or_insert(Rule { base: Action::K, overrides: Vec::new() });
        if option_s.is_empty() {
            rule.base = action;
        } else if let Some(opt) = Opt::from_name(option_s) {
            rule.overrides.push((opt, action));
        }
    }
    map
}

/// Parses a concrete `(gggg,eeee)` tag; `None` for a wildcard/group row.
fn parse_tag(s: &str) -> Option<Tag> {
    let inner = s.strip_prefix('(')?.strip_suffix(')')?;
    let (g, e) = inner.split_once(',')?;
    if g.contains(['x', 'g']) || e.contains(['x', 'g']) {
        return None;
    }
    Some(Tag(u16::from_str_radix(g, 16).ok()?, u16::from_str_radix(e, 16).ok()?))
}

/// Run-level UID and per-patient state, so replacements are consistent across
/// every object de-identified in one run. UIDs reuse the export's derivation.
#[derive(Debug, Clone, Default)]
pub struct UidMap {
    salt: String,
    uids: BTreeMap<String, String>,
    patients: BTreeMap<String, Patient>,
}

#[derive(Debug, Clone)]
struct Patient {
    id: String,
    offset_days: i64,
}

impl UidMap {
    /// A map seeding replacements with `salt`.
    pub fn new(salt: impl Into<String>) -> Self {
        Self { salt: salt.into(), uids: BTreeMap::new(), patients: BTreeMap::new() }
    }

    /// The consistent replacement of `original` (the export's salted derivation).
    pub fn uid(&mut self, original: &str) -> String {
        if let Some(u) = self.uids.get(original) {
            return u.clone();
        }
        let u = uid_from(&["ferrum-uid", &self.salt, original]);
        self.uids.insert(original.to_owned(), u.clone());
        u
    }

    fn patient(&mut self, key: &str) -> Patient {
        if let Some(p) = self.patients.get(key) {
            return p.clone();
        }
        let id = uid_from(&["ferrum-patient", &self.salt, key]);
        let p = Patient { id, offset_days: date_offset(&self.salt, key) };
        self.patients.insert(key.to_owned(), p.clone());
        p
    }
}

/// A consistent per-patient date shift in `[1, 3650]` days (≈ up to 10 years),
/// derived from the salted patient key so it is stable across a run.
fn date_offset(salt: &str, key: &str) -> i64 {
    let mut h = Sha256::new();
    h.update(b"ferrum-date-offset");
    h.update([0]);
    h.update(salt.as_bytes());
    h.update([0]);
    h.update(key.as_bytes());
    let d = h.finalize();
    let n = u64::from_be_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]);
    1 + (n % 3650) as i64
}

/// The action the engine applies once compound codes and options are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effective {
    D,
    Z,
    X,
    U,
    C,
    K,
}

/// The effective action for an attribute and the option governing a `C`.
struct Resolved {
    action: Effective,
    govern: Option<Opt>,
}

fn resolve(rule: Option<&Rule>, opts: &ProfileOptions, vr: VR) -> Resolved {
    let Some(rule) = rule else {
        return Resolved { action: Effective::K, govern: None };
    };
    let mut c_opts: Vec<Opt> = Vec::new();
    let mut has_k = false;
    for (opt, action) in &rule.overrides {
        if !opts.enabled(*opt) {
            continue;
        }
        match action {
            Action::C => c_opts.push(*opt),
            Action::K => has_k = true,
            _ => {}
        }
    }
    if !c_opts.is_empty() {
        let govern = pick_clean_govern(&c_opts, vr);
        return Resolved { action: Effective::C, govern: Some(govern) };
    }
    if has_k {
        return Resolved { action: Effective::K, govern: None };
    }
    Resolved { action: rule.base.effective(), govern: None }
}

/// Among enabled `C` options, dates govern date VRs, text-cleaning governs the rest.
fn pick_clean_govern(c_opts: &[Opt], vr: VR) -> Opt {
    let date = matches!(vr, VR::DA | VR::TM | VR::DT);
    let wanted = if date { Opt::DatesModified } else { Opt::Descriptors };
    c_opts.iter().copied().find(|o| *o == wanted).unwrap_or(c_opts[0])
}

struct Ctx<'a> {
    opts: &'a ProfileOptions,
    uids: &'a mut UidMap,
    patient_id: String,
    offset_days: i64,
    identifiers: Vec<String>,
}

/// De-identifies `obj` in place under `profile`, recording replacements in
/// `uids` so they stay consistent across the run. Pixel data and the attributes
/// the profile keeps are retained; the method attributes (E.1.1 item 6) are added.
pub fn deidentify(obj: &mut InMemDicomObject, profile: &Profile, uids: &mut UidMap) {
    let patient = uids.patient(&patient_key(obj));
    let mut ctx = Ctx {
        opts: &profile.options,
        uids,
        patient_id: patient.id,
        offset_days: patient.offset_days,
        identifiers: collect_identifiers(obj),
    };
    deidentify_object(obj, &mut ctx, 0);
    add_method_attributes(obj, &profile.options);
}

/// De-identifies a whole file object: the data set under `profile` and the
/// File Meta Information replaced consistently (E.1.1 item 7, the writer's
/// responsibility). The media storage SOP class/instance UIDs follow the
/// de-identified data set, the implementation identity becomes FERRUM's, and
/// the source's device identity in the meta group (application entity titles)
/// and private meta information are removed — the titles kept only under the
/// Retain Device Identity option. The transfer syntax and the meta information
/// version are kept; the group length is recomputed.
pub fn deidentify_file(file: &mut FileDicomObject<InMemDicomObject>, profile: &Profile, uids: &mut UidMap) {
    deidentify(file, profile, uids);
    let sop_class = str_of(file, tags::SOP_CLASS_UID).unwrap_or_default();
    let sop_instance = str_of(file, tags::SOP_INSTANCE_UID).unwrap_or_default();
    let keep_device = profile.options.retain_device_identity;
    file.update_meta(|meta| {
        meta.media_storage_sop_class_uid = sop_class;
        meta.media_storage_sop_instance_uid = sop_instance;
        meta.implementation_class_uid = implementation_class_uid().to_owned();
        meta.implementation_version_name = Some(format!("FERRUM {}", env!("CARGO_PKG_VERSION")));
        if !keep_device {
            meta.source_application_entity_title = None;
            meta.sending_application_entity_title = None;
            meta.receiving_application_entity_title = None;
        }
        meta.private_information_creator_uid = None;
        meta.private_information = None;
    });
}

/// FERRUM's implementation class UID for de-identified files: a stable UUID
/// derivation (the export's `2.25` root), not the source writer's identity.
fn implementation_class_uid() -> &'static str {
    static UID: OnceLock<String> = OnceLock::new();
    UID.get_or_init(|| uid_from(&["ferrum-implementation-class"]))
}

fn deidentify_object(obj: &mut InMemDicomObject, ctx: &mut Ctx<'_>, depth: usize) {
    let tags: Vec<Tag> = obj.iter().map(|e| e.tag()).collect();
    for tag in tags {
        apply_to_element(obj, tag, ctx, depth);
    }
}

fn apply_to_element(obj: &mut InMemDicomObject, tag: Tag, ctx: &mut Ctx<'_>, depth: usize) {
    let Some(elem) = obj.take(tag) else { return };
    let vr = elem.vr();
    let rule = rules().get(&tag);
    if removed_by_group_rule(tag, rule.is_some()) {
        return;
    }
    let resolved = resolve(rule, ctx.opts, vr);
    if vr == VR::SQ {
        apply_sequence(obj, tag, elem, &resolved, ctx, depth);
        return;
    }
    let out = match resolved.action {
        Effective::X => None,
        Effective::Z => Some(zero_or_dummy(tag, vr, ctx)),
        Effective::D => Some(placeholder(tag, vr, ctx)),
        Effective::U => Some(uid_elem(tag, &elem, ctx)),
        Effective::C => Some(clean_or_shift(tag, vr, &elem, resolved.govern, ctx)),
        Effective::K => Some(elem),
    };
    if let Some(e) = out {
        obj.put(e);
    }
}

fn apply_sequence(
    obj: &mut InMemDicomObject,
    tag: Tag,
    elem: Elem,
    resolved: &Resolved,
    ctx: &mut Ctx<'_>,
    depth: usize,
) {
    match resolved.action {
        Effective::X => return,
        Effective::Z => {
            obj.put(Elem::empty(tag, VR::SQ));
            return;
        }
        // D (and K/C/U) on a sequence applies to the sequence and all of its
        // contents (PS3.15 Note 2.1): keep the structure and de-identify the
        // contents recursively below — never collapse a D sequence to one empty
        // item, which would destroy e.g. an exported SR ContentSequence.
        _ => {}
    }
    if depth >= MAX_DEPTH {
        obj.put(Elem::empty(tag, VR::SQ));
        return;
    }
    let items = elem.items().map(|s| s.to_vec()).unwrap_or_default();
    let mut new_items = Vec::with_capacity(items.len());
    for mut item in items {
        deidentify_object(&mut item, ctx, depth + 1);
        new_items.push(item);
    }
    obj.put(DataElement::new(tag, VR::SQ, DataSetSequence::from(new_items)));
}

/// Table E.1-1 wildcard rows and E.1.1: private (odd group), curve (`50xx`) and
/// overlay comments/data (`60xx,4000`/`60xx,3000`) are removed. Group `0004`
/// (directory information) is removed except for the one attribute the table
/// lists itself (`ReferencedSOPInstanceUIDInFile`), whose table action governs.
fn removed_by_group_rule(tag: Tag, in_table: bool) -> bool {
    let g = tag.group();
    if (g & 1) == 1 {
        return true;
    }
    if (g & 0xFF00) == 0x5000 {
        return true;
    }
    if (g & 0xFF00) == 0x6000 && matches!(tag.element(), 0x4000 | 0x3000) {
        return true;
    }
    if g == 0x0004 && !in_table {
        return true;
    }
    false
}

/// `Z`: zero-length, except the two documented non-empty dummies.
fn zero_or_dummy(tag: Tag, vr: VR, ctx: &Ctx<'_>) -> Elem {
    if tag == tags::PATIENT_NAME {
        return DataElement::new(tag, VR::PN, PrimitiveValue::from("ANONYMOUS"));
    }
    if tag == tags::PATIENT_ID {
        return DataElement::new(tag, vr, PrimitiveValue::from(ctx.patient_id.as_str()));
    }
    Elem::empty(tag, vr)
}

/// `D`: a non-empty dummy consistent with the VR.
fn placeholder(tag: Tag, vr: VR, ctx: &Ctx<'_>) -> Elem {
    let value = match vr {
        VR::DA => PrimitiveValue::from("19000101"),
        VR::TM => PrimitiveValue::from("000000"),
        VR::DT => PrimitiveValue::from("19000101000000"),
        VR::AS => PrimitiveValue::from("000Y"),
        VR::CS | VR::AE => PrimitiveValue::from("ANON"),
        VR::UR => PrimitiveValue::from("http://localhost/"),
        VR::UI => PrimitiveValue::from(uid_from(&["ferrum-anon", &ctx.opts.uid_salt, &format!("{tag:?}")]).as_str()),
        VR::OB | VR::UN => PrimitiveValue::from(vec![0u8]),
        _ => PrimitiveValue::from("ANONYMOUS"),
    };
    DataElement::new(tag, vr, value)
}

/// `U`: a consistent pseudonymous UID via the run's [`UidMap`].
fn uid_elem(tag: Tag, elem: &Elem, ctx: &mut Ctx<'_>) -> Elem {
    let original = elem.to_str().map(|s| s.trim().to_owned()).unwrap_or_default();
    let replacement = ctx.uids.uid(&original);
    DataElement::new(tag, VR::UI, PrimitiveValue::from(replacement.as_str()))
}

/// `C`: shift dates under `retain_dates_modified`, else scrub free text.
fn clean_or_shift(tag: Tag, vr: VR, elem: &Elem, govern: Option<Opt>, ctx: &Ctx<'_>) -> Elem {
    if govern == Some(Opt::DatesModified) {
        return shift_date(tag, vr, elem, ctx.offset_days);
    }
    match vr {
        VR::DA | VR::TM | VR::DT => shift_date(tag, vr, elem, ctx.offset_days),
        VR::SQ => elem.clone(),
        VR::OB | VR::UN | VR::CS => Elem::empty(tag, vr),
        _ => {
            let text = elem.to_str().map(|s| s.into_owned()).unwrap_or_default();
            let cleaned = clean_text(&text, &ctx.identifiers);
            DataElement::new(tag, vr, PrimitiveValue::from(cleaned.as_str()))
        }
    }
}

/// Shifts `DA`/`DT` dates by `offset` days, keeps `TM` times (E.3.6 conformance
/// choice); other VRs marked `C` by the dates option are emptied conservatively.
fn shift_date(tag: Tag, vr: VR, elem: &Elem, offset: i64) -> Elem {
    let s = elem.to_str().map(|s| s.trim().to_owned()).unwrap_or_default();
    let out = match vr {
        VR::TM => s,
        VR::DA => shift_da(&s, offset),
        VR::DT => shift_dt(&s, offset),
        _ => String::new(),
    };
    DataElement::new(tag, vr, PrimitiveValue::from(out.as_str()))
}

fn shift_da(s: &str, offset: i64) -> String {
    let Some((y, m, d)) = parse_ymd(s) else { return "19000101".to_owned() };
    let days = days_from_civil(y, m, d) + offset;
    let (y, m, d) = civil_from_days(days);
    format!("{:04}{:02}{:02}", y.clamp(1, 9999), m, d)
}

fn shift_dt(s: &str, offset: i64) -> String {
    if s.len() < 8 {
        return "19000101000000".to_owned();
    }
    format!("{}{}", shift_da(&s[..8], offset), &s[8..])
}

fn parse_ymd(s: &str) -> Option<(i64, i64, i64)> {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y = s[0..4].parse().ok()?;
    let m = s[4..6].parse().ok()?;
    let d = s[6..8].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Days since 1970-01-01 from a civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Civil date from days since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Scrubs the object's own identifier values and long digit runs from free text.
fn clean_text(s: &str, identifiers: &[String]) -> String {
    let mut out = s.to_owned();
    for id in identifiers {
        if id.len() >= 2 {
            out = replace_ignore_case(&out, id, " ");
        }
    }
    collapse_spaces(&strip_long_digits(&out))
}

fn replace_ignore_case(s: &str, needle: &str, replacement: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let nlower = needle.to_ascii_lowercase();
    let mut out = String::new();
    let mut rest = lower.as_str();
    let mut original = s;
    while let Some(pos) = rest.find(&nlower) {
        out.push_str(&original[..pos]);
        out.push_str(replacement);
        original = &original[pos + needle.len()..];
        rest = &rest[pos + nlower.len()..];
    }
    out.push_str(original);
    out
}

/// Removes maximal ASCII-digit runs of length ≥ 6 (IDs, compact dates).
fn strip_long_digits(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            let mut run = String::from(c);
            while let Some(&n) = chars.peek() {
                if n.is_ascii_digit() {
                    run.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            if run.len() < 6 {
                out.push_str(&run);
            } else {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn collapse_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The patient identity key (ID + name) used to seed per-patient replacements.
fn patient_key(obj: &InMemDicomObject) -> String {
    let id = str_of(obj, tags::PATIENT_ID).unwrap_or_default();
    let name = str_of(obj, tags::PATIENT_NAME).unwrap_or_default();
    format!("{id}|{name}")
}

/// The object's own identifier values, used to scrub descriptors.
fn collect_identifiers(obj: &InMemDicomObject) -> Vec<String> {
    const SOURCES: [Tag; 12] = [
        tags::PATIENT_NAME,
        tags::PATIENT_ID,
        tags::ACCESSION_NUMBER,
        tags::PATIENT_BIRTH_DATE,
        tags::STUDY_DATE,
        tags::STUDY_TIME,
        tags::REFERRING_PHYSICIAN_NAME,
        tags::OPERATORS_NAME,
        tags::PERFORMING_PHYSICIAN_NAME,
        tags::NAME_OF_PHYSICIANS_READING_STUDY,
        tags::PHYSICIANS_OF_RECORD,
        tags::INSTITUTION_NAME,
    ];
    let mut out = Vec::new();
    for tag in SOURCES {
        let Some(v) = str_of(obj, tag).filter(|v| !v.is_empty()) else { continue };
        for part in v.split('^') {
            if part.len() >= 2 {
                out.push(part.to_owned());
            }
        }
        if v.len() >= 2 {
            out.push(v);
        }
    }
    out
}

fn add_method_attributes(obj: &mut InMemDicomObject, opts: &ProfileOptions) {
    obj.put(DataElement::new(tags::PATIENT_IDENTITY_REMOVED, VR::CS, PrimitiveValue::from("YES")));
    obj.put(DataElement::new(tags::DEIDENTIFICATION_METHOD, VR::LO, PrimitiveValue::from(method_text(opts).as_str())));
    obj.put(DataElement::new(
        tags::DEIDENTIFICATION_METHOD_CODE_SEQUENCE,
        VR::SQ,
        DataSetSequence::from(method_codes(opts)),
    ));
    if opts.retain_dates_modified {
        obj.put(DataElement::new(
            tags::LONGITUDINAL_TEMPORAL_INFORMATION_MODIFIED,
            VR::CS,
            PrimitiveValue::from("MODIFIED"),
        ));
    }
}

fn method_text(opts: &ProfileOptions) -> String {
    let mut s = "FERRUM: PS3.15 Basic Application Level Confidentiality Profile".to_owned();
    let enabled = opts.enabled_opts();
    if !enabled.is_empty() {
        let names: Vec<&str> = enabled.iter().map(|o| o.as_str()).collect();
        s.push_str(&format!("; options: {}", names.join(", ")));
    }
    s
}

fn method_codes(opts: &ProfileOptions) -> Vec<InMemDicomObject> {
    let mut codes = vec![code_item("113100", "Basic Application Confidentiality Profile")];
    codes.extend(opts.enabled_opts().iter().map(|o| {
        let (value, meaning) = o.code();
        code_item(value, meaning)
    }));
    codes
}

fn code_item(value: &str, meaning: &str) -> InMemDicomObject {
    InMemDicomObject::from_element_iter([
        DataElement::new(tags::CODE_VALUE, VR::SH, PrimitiveValue::from(value)),
        DataElement::new(tags::CODING_SCHEME_DESIGNATOR, VR::SH, PrimitiveValue::from("DCM")),
        DataElement::new(tags::CODE_MEANING, VR::LO, PrimitiveValue::from(meaning)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for &(y, m, d) in &[(2024, 4, 28), (2000, 2, 29), (1999, 12, 31), (2024, 1, 1)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(shift_da("20240428", 1), "20240429");
        assert_eq!(shift_da("20240228", 1), "20240229");
        assert_eq!(shift_da("20230228", 1), "20230301");
        assert_eq!(shift_dt("20240428102417", 1), "20240429102417");
    }

    #[test]
    fn scrub_removes_identifiers_and_long_digits() {
        let ids = vec!["DOE".to_owned(), "JOHN".to_owned(), "123456".to_owned()];
        let cleaned = clean_text("CT chest for DOE^JOHN id 123456 on 20240428", &ids);
        assert!(!cleaned.contains("DOE"));
        assert!(!cleaned.contains("JOHN"));
        assert!(!cleaned.contains("123456"));
        assert!(!cleaned.contains("20240428"));
        assert!(cleaned.contains("CT chest"));
        assert_eq!(strip_long_digits("abc 12345 def 1234567"), "abc 12345 def  ");
    }

    #[test]
    fn unknown_option_is_refused() {
        let mut opts = ProfileOptions::default();
        assert!(opts.enable("retain_uids").is_ok());
        assert!(opts.retain_uids);
        assert_eq!(opts.enable("retain_everything"), Err(DeidError::UnknownOption("retain_everything".into())));
    }

    #[test]
    fn compound_actions_take_the_primary_branch() {
        assert_eq!(Action::XZ.effective(), Effective::X);
        assert_eq!(Action::XZD.effective(), Effective::X);
        assert_eq!(Action::XZU.effective(), Effective::X);
        assert_eq!(Action::ZD.effective(), Effective::Z);
        assert_eq!(Action::D.effective(), Effective::D);
    }
}
