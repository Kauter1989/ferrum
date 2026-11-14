//! The view-mode names, on the host (F1.1-a, F1.1-c).
//!
//! The bug these guard against: an unknown mode name was applied silently and
//! the canvas drew one 2D slice to a person who had asked for MPR (F1).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/mode.rs"]
mod mode;

use mode::Mode;

// covers F1.1-a
#[test]
fn an_unknown_mode_name_is_refused_and_says_which() {
    let err = Mode::parse("mprr").unwrap_err();
    assert!(err.contains("mprr"), "the error does not name the value: {err}");
    assert!(
        err.contains("slice") && err.contains("volume") && err.contains("mpr"),
        "the error does not list the modes: {err}"
    );

    for bad in ["", " ", "3d", "2d", "isosurface", "SLICE2D", "transfer_function"] {
        assert!(Mode::parse(bad).is_err(), "{bad:?} was accepted as a mode");
    }
}

// covers F1.1-c
#[test]
fn every_name_the_panel_can_send_is_accepted() {
    // The three strings `integrations/dsh/plugin/lib/client.js` produces.
    assert_eq!(Mode::parse("slice"), Ok(Mode::Slice));
    assert_eq!(Mode::parse("volume"), Ok(Mode::Volume));
    assert_eq!(Mode::parse("mpr"), Ok(Mode::Mpr));

    // The name round-trips, so the panel and the module cannot drift apart.
    for m in [Mode::Slice, Mode::Volume, Mode::Mpr] {
        assert_eq!(Mode::parse(m.name()), Ok(m));
    }
}

// covers F1.1-a
#[test]
fn case_and_spacing_do_not_change_the_mode() {
    // The panel sends exact lowercase names, but the sibling setters
    // (`set_render_mode`, `set_slice`, `set_ct_preset`) fold case and trim, so
    // the mode does the same rather than being the odd one out.
    for (text, expected) in [
        ("MPR", Mode::Mpr),
        (" MPR ", Mode::Mpr),
        ("mpr ", Mode::Mpr),
        ("Volume", Mode::Volume),
        (" SLICE\t", Mode::Slice),
    ] {
        assert_eq!(Mode::parse(text), Ok(expected), "{text:?} parsed wrongly");
    }
}
