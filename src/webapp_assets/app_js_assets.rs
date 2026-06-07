macro_rules! qcold_app_js_assets {
    ($callback:ident) => {
        $callback! {
            "api.js",
            "init_parse.js",
            "queue.js",
            "terminal.js",
            "events.js",
            "queue_scroll.js",
            "queue_transcript_lookup.js",
            "events_bootstrap.js",
        }
    };
}
