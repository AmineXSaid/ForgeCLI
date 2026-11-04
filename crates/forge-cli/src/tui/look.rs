//! The terminal UI's styles (`/theme <style>`, the `tuiStyle` setting): how
//! the front page, the chat's lines and the prompt are laid out. The colours
//! (`/theme dark|light|none`) are separate and apply to every style.

/// One style of the terminal UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    /// A framed welcome with the anvil and what is connected; a timeline of steps.
    #[default]
    Forgeframe,
    /// Centred, the name as hot metal; quiet ✓ steps and a glowing prompt.
    Molten,
    /// A card with the anvil and the session; turns headed `you` and `forge`.
    Anvil,
    /// Aligned left, past work numbered; times and voices down the side.
    Ledger,
    /// Readouts of the session and a header bar; approvals over the thread.
    Status,
    /// One prompt in the middle; plain steps and answers.
    Bare,
}

impl Look {
    pub const ALL: [Look; 6] = [Look::Forgeframe, Look::Molten, Look::Anvil, Look::Ledger, Look::Status, Look::Bare];

    /// The style a setting names, if any (case does not matter).
    pub fn named(s: &str) -> Option<Look> {
        Look::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Look::Forgeframe => "forgeframe",
            Look::Molten => "molten",
            Look::Anvil => "anvil",
            Look::Ledger => "ledger",
            Look::Status => "status",
            Look::Bare => "bare",
        }
    }

    /// Steps are written as plain, quiet lines (`read src/a.rs`) rather than with a dot and a bold verb.
    pub fn plain_steps(self) -> bool {
        matches!(self, Look::Ledger | Look::Bare)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_found_by_name() {
        assert_eq!(Look::named("Molten"), Some(Look::Molten));
        assert_eq!(Look::named(" bare "), Some(Look::Bare));
        assert_eq!(Look::named("neon"), None);
        assert!(Look::ALL.iter().all(|l| Look::named(l.name()) == Some(*l)));
        // The names match the /theme command's list.
        let core: Vec<&str> = forge_core::commands::STYLES.iter().map(|(n, _)| *n).collect();
        let ui: Vec<&str> = Look::ALL.iter().map(|l| l.name()).collect();
        assert_eq!(core, ui);
    }
}
