//! Reading the patient identifiers of a source, for the host privacy scan
//! ([#74](https://github.com/Kauter1989/ferrum/issues/74)).
//!
//! The values are read here, inside FERRUM's I/O layer, and handed to the
//! scan (`ferrum_agent::identifiers`), which returns positions and kinds
//! only. Nothing in this module writes identifiers anywhere.

use std::path::Path;

use dicom_dictionary_std::tags;
use dicom_object::OpenFileOptions;
use ferrum_domain::PatientIdentifiers;

use crate::error::IoError;

/// The patient identifiers of `path`: a DICOM file, a folder (the first
/// readable DICOM file in name order — the tags are the series'; symlinks
/// are skipped, as the source walk does), or a NIfTI file, which carries
/// none (every field empty).
pub fn read_patient_identifiers(path: &Path) -> Result<PatientIdentifiers, IoError> {
    if path.is_dir() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(path)
            .map_err(|e| IoError::os(path, e))?
            .filter_map(|e| e.ok())
            // file_type() does not follow the link: a planted symlink must
            // not lead the reader outside the operator's roots (#82 class)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.path())
            .collect();
        files.sort();
        for file in files {
            if let Ok(ids) = read_file(&file) {
                return Ok(ids);
            }
        }
        return Ok(PatientIdentifiers::default());
    }
    read_file(path)
}

/// The patient identifiers of a workspace's recorded series files: the
/// first file that reads. The caller checks every path against the
/// operator's roots and hash-verifies the sources first (as `Study::load`
/// does); unlike the folder walk, this never discovers new files. When no
/// file reads, the error is returned — not empty identifiers (fail closed).
pub fn read_patient_identifiers_of_files(files: &[std::path::PathBuf]) -> Result<PatientIdentifiers, IoError> {
    let mut last = None;
    for file in files {
        match read_file(file) {
            Ok(ids) => return Ok(ids),
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(e),
        None => Ok(PatientIdentifiers::default()),
    }
}

/// One file: DICOM headers stop before the pixel data; a NIfTI file has no
/// patient identifiers.
fn read_file(path: &Path) -> Result<PatientIdentifiers, IoError> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    if ext == "nii" || ext == "gz" {
        return Ok(PatientIdentifiers::default());
    }
    let obj =
        OpenFileOptions::new().read_until(tags::PIXEL_DATA).open_file(path).map_err(|e| IoError::parse(path, e))?;
    Ok(crate::dicom::header::patient_identifiers(&obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 27.1-a
    #[test]
    fn a_nifti_file_carries_no_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.nii");
        std::fs::write(&path, b"not a dicom file").unwrap();
        let ids = read_patient_identifiers(&path).unwrap();
        assert_eq!(ids, PatientIdentifiers::default());
    }

    // covers 27.1-a
    #[test]
    fn a_folder_without_dicom_carries_no_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"hello").unwrap();
        let ids = read_patient_identifiers(dir.path()).unwrap();
        assert_eq!(ids, PatientIdentifiers::default());
    }

    // covers 27.1-a (security review of #110: the VR maxima bound the
    // matcher's input, so a crafted huge value cannot exhaust the host)
    #[test]
    fn long_values_are_capped_to_the_vr_maxima() {
        use dicom_core::{DataElement, PrimitiveValue, VR};
        use dicom_dictionary_std::tags;
        use dicom_object::InMemDicomObject;
        let long = "A".repeat(5000);
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(tags::PATIENT_NAME, VR::PN, PrimitiveValue::from(format!("{long}^{long}"))),
            DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from(long.clone())),
            DataElement::new(tags::ACCESSION_NUMBER, VR::SH, PrimitiveValue::from(long.clone())),
            DataElement::new(tags::PATIENT_BIRTH_DATE, VR::DA, PrimitiveValue::from("19700101")),
        ]);
        let ids = crate::dicom::header::patient_identifiers(&obj);
        assert_eq!(ids.patient_id.chars().count(), 64);
        assert_eq!(ids.accession_number.chars().count(), 16);
        assert_eq!(ids.birth_date, "19700101");
        for comp in ids.name.split('^') {
            assert_eq!(comp.chars().count(), 64);
        }
    }
}
