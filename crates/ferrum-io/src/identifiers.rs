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
/// readable DICOM file in name order — the tags are the series'), or a
/// NIfTI file, which carries none (every field empty).
pub fn read_patient_identifiers(path: &Path) -> Result<PatientIdentifiers, IoError> {
    if path.is_dir() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(path)
            .map_err(|e| IoError::os(path, e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
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

    #[test]
    fn a_nifti_file_carries_no_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.nii");
        std::fs::write(&path, b"not a dicom file").unwrap();
        let ids = read_patient_identifiers(&path).unwrap();
        assert_eq!(ids, PatientIdentifiers::default());
    }

    #[test]
    fn a_folder_without_dicom_carries_no_identifiers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"hello").unwrap();
        let ids = read_patient_identifiers(dir.path()).unwrap();
        assert_eq!(ids, PatientIdentifiers::default());
    }
}
