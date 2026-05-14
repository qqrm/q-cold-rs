#[cfg(test)]
mod terminal_send_tests {
    use super::*;

    #[test]
    fn terminal_paste_submit_delay_waits_for_large_multiline_pastes() {
        assert_eq!(
            terminal_paste_submit_delay("short prompt"),
            Duration::from_millis(100)
        );
        assert_eq!(
            terminal_paste_submit_delay("line one\nline two"),
            Duration::from_millis(1500)
        );
        assert_eq!(
            terminal_paste_submit_delay(&"x".repeat(1025)),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn terminal_output_detects_unsent_pasted_packet_at_prompt() {
        let output = "\n> [Pasted Content 1024 chars][Pasted Content 512 chars]\n";
        assert!(terminal_output_has_pending_paste(output));
        let expanded = "\nEND_Q-COLD_TASK_PACKET\n\n  gpt-5.5 xhigh · ~/repo\n";
        assert!(terminal_output_has_pending_paste(expanded));
        let accepted = "\nEND_Q-COLD_TASK_PACKET\n\n• Working (3s • esc to interrupt)\n";
        assert!(!terminal_output_has_pending_paste(accepted));
        assert!(!terminal_output_has_pending_paste("* Ran cargo test\n"));
    }

    #[test]
    fn queue_input_waits_for_idle_codex_prompt() {
        let booting = "\n• Booting MCP server: codex_apps (0s • esc to interrupt)\n\
                       › Find and fix a bug in @filename\n\
                       gpt-5.5 high · ~/repo\n";
        assert!(!terminal_output_ready_for_queue_input(booting));

        let idle = "\n› Find and fix a bug in @filename\n\
                    gpt-5.5 high · ~/repo\n";
        assert!(terminal_output_ready_for_queue_input(idle));

        let pending_paste = "\n› [Pasted Content 2048 chars][Pasted Content 1024 chars]\n\
                             gpt-5.5 high · ~/repo\n";
        assert!(!terminal_output_ready_for_queue_input(pending_paste));
    }
}
