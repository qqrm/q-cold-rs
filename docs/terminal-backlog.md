# Terminal Backlog

Q-COLD supports `tmux` as the default attachable terminal multiplexer and an
experimental `zellij` backend selected with `QCOLD_TERMINAL_BACKEND=zellij`.
`tmux` remains the default because it is the most proven backend in the current
Q-COLD command surface.

Before making `zellij` the default, run a focused terminal backend hardening
pass:

- Exercise `zellij` with real `c2`/Codex sessions over long runs.
- Verify session discovery, attach from a normal local terminal, ANSI
  transcript capture, large paste delivery, pane input, and predictable exit
  behavior after agent `/q`.
- Evaluate Zellij's built-in `zellij web` surface for possible integration or
  replacement of Q-COLD's current terminal view.
- Compare the browser integration cost against keeping `tmux` and against a
  Q-COLD-native PTY manager implemented in Rust.
- Do not promote `zellij` to the default until it preserves both operator
  flows: local terminal attach and Q-COLD web terminal control.

The likely long-term target is a Q-COLD-owned Rust PTY manager if the web
terminal becomes a primary surface. `zellij` is now a real experimental backend
and should be treated as the preferred migration candidate unless the hardening
pass exposes a blocker.
