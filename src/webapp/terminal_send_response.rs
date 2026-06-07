impl TerminalSendResponse {
    fn success(output: impl Into<String>) -> Self {
        Self {
            ok: true,
            output: output.into(),
            queue_graph: None,
        }
    }

    fn success_with_queue_graph(
        output: impl Into<String>,
        queue_graph: QueueGraphDiagnostics,
    ) -> Self {
        Self {
            ok: true,
            output: output.into(),
            queue_graph: Some(queue_graph),
        }
    }

    fn failure(output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
            queue_graph: None,
        }
    }

    fn failure_with_queue_graph(
        output: impl Into<String>,
        queue_graph: QueueGraphDiagnostics,
    ) -> Self {
        Self {
            ok: false,
            output: output.into(),
            queue_graph: Some(queue_graph),
        }
    }
}
