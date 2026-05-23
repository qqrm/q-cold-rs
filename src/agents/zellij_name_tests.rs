#[cfg(test)]
mod zellij_name_tests {
    use super::*;

    #[test]
    fn zellij_layout_uses_requested_pane_name() {
        let layout = zellij_layout("client migration", "printf ok").unwrap();

        assert!(layout.contains("pane name=\"client migration\" command=\"sh\""));
        assert!(!layout.contains("pane name=\"c1-"));
    }

    #[test]
    fn zellij_pane_name_is_compacted_and_limited() {
        assert_eq!(
            clean_zellij_pane_name(Some("  client   migration  ")).unwrap().as_deref(),
            Some("client migration")
        );
        assert_eq!(
            clean_zellij_pane_name(Some(&"x".repeat(90)))
                .unwrap()
                .unwrap()
                .chars()
                .count(),
            80
        );
        assert!(clean_zellij_pane_name(Some(" \t ")).is_err());
    }
}
