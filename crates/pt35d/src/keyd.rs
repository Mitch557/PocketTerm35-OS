//! Face buttons for a game, through keyd.
//!
//! A game reads the keyboard, and the keys the pt35 firmware sends for the face
//! buttons mean nothing to most of them: RetroArch knows no key past F15. keyd
//! already holds the keyboard, so while such an app is in front it turns each
//! button into the key the app's profile names. sway then never sees the
//! button's own key, so the shell's bindings stay as they are and none is taken
//! off a held key. keyd releases a held key as the key it pressed, even when the
//! map changes under it.
//!
//! keyd's socket belongs to the `keyd` group.

use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::process::{Command, Stdio};

/// keyd's binding lines for a profile's `buttons` table. A button that is not
/// there is left out: `AppProfile::validate` names it.
pub fn bindings(buttons: &BTreeMap<String, String>) -> Vec<String> {
    buttons
        .iter()
        .filter_map(|(button, key)| {
            pt35_common::apps::button_key(button).map(|from| format!("{from} = {key}"))
        })
        .collect()
}

/// Replace every binding keyd holds with `bindings`. Empty gives the buttons
/// back to the shell.
pub fn apply(bindings: &[String]) -> Result<()> {
    // Debian ships keyd as keyd.rvaiya: `keyd` is another package's name there.
    for bin in ["keyd.rvaiya", "keyd"] {
        let out = Command::new(bin)
            .args(["bind", "reset"])
            .args(bindings)
            .stdin(Stdio::null())
            .output();
        match out {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => bail!(
                "{bin} bind: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => bail!("{bin}: {e}"),
        }
    }
    bail!("keyd is not installed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_button_becomes_the_key_its_firmware_sends() {
        let buttons = BTreeMap::from([
            ("a".to_string(), "x".to_string()),
            ("start".to_string(), "enter".to_string()),
            ("up".to_string(), "w".to_string()),
        ]);
        assert_eq!(bindings(&buttons), vec!["f18 = x", "f22 = enter"]);
    }
}
