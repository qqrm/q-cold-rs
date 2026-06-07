#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QueueGraphDiagnostics {
    pub(crate) ok: bool,
    pub(crate) execution_mode: String,
    pub(crate) diagnostics: Vec<QueueGraphDiagnostic>,
    pub(crate) items: Vec<QueueGraphItemDiagnostic>,
}

impl QueueGraphDiagnostics {
    fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == QueueGraphDiagnosticSeverity::Error)
    }

    fn validation_message(&self) -> String {
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == QueueGraphDiagnosticKind::Cycle)
        {
            return "queue dependency graph contains a cycle".to_string();
        }
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == QueueGraphDiagnosticKind::DuplicateId)
        {
            return "queue dependency graph contains duplicate item ids".to_string();
        }
        if self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == QueueGraphDiagnosticKind::MissingId)
        {
            return "queue dependency graph contains items without ids".to_string();
        }
        "queue dependency graph is invalid".to_string()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QueueGraphDiagnostic {
    pub(crate) kind: QueueGraphDiagnosticKind,
    pub(crate) severity: QueueGraphDiagnosticSeverity,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dependency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dependency_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) requested_wave_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) requested_wave_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) canonical_wave_index: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub(crate) cycle: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QueueGraphDiagnosticKind {
    InvalidDependency,
    MissingDependency,
    MissingId,
    DuplicateId,
    Cycle,
    WaveConflict,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QueueGraphDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QueueGraphItemDiagnostic {
    pub(crate) id: String,
    pub(crate) slug: String,
    pub(crate) position: i64,
    pub(crate) depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) requested_wave_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) requested_wave_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) canonical_wave_index: Option<usize>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct QueueGraphItemMetadata {
    wave_id: Option<String>,
    wave_index: Option<usize>,
}

impl QueueGraphItemMetadata {
    fn from_run_request(request: &QueueRunItemRequest) -> Self {
        Self {
            wave_id: clean_queue_graph_wave_id(request.wave_id.as_deref()),
            wave_index: request.wave_index,
        }
    }

    fn from_update_request(request: &QueueUpdateItemRequest) -> Self {
        Self {
            wave_id: clean_queue_graph_wave_id(request.wave_id.as_deref()),
            wave_index: request.wave_index,
        }
    }
}

#[derive(Clone, Debug)]
struct QueueGraphValidationError {
    diagnostics: QueueGraphDiagnostics,
    message: String,
}

impl QueueGraphValidationError {
    fn from_diagnostics(diagnostics: QueueGraphDiagnostics) -> Self {
        Self {
            message: diagnostics.validation_message(),
            diagnostics,
        }
    }
}

impl std::fmt::Display for QueueGraphValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for QueueGraphValidationError {}

fn queue_graph_diagnostics_from_error(err: &anyhow::Error) -> Option<QueueGraphDiagnostics> {
    err.downcast_ref::<QueueGraphValidationError>()
        .map(|err| err.diagnostics.clone())
}

#[cfg(test)]
fn normalize_queue_dependencies(
    execution_mode: impl Into<state::QueueExecutionMode>,
    items: &mut [state::QueueItemRow],
) -> Result<QueueGraphDiagnostics> {
    normalize_queue_dependencies_with_metadata(execution_mode, items, &[])
}

fn normalize_queue_dependencies_with_metadata(
    execution_mode: impl Into<state::QueueExecutionMode>,
    items: &mut [state::QueueItemRow],
    metadata: &[QueueGraphItemMetadata],
) -> Result<QueueGraphDiagnostics> {
    let execution_mode = execution_mode.into();
    if !execution_mode.is_graph() {
        for item in items.iter_mut() {
            item.depends_on.clear();
        }
        return Ok(queue_graph_diagnostics(
            &execution_mode,
            items,
            metadata,
            None,
            Vec::new(),
        ));
    }

    let mut diagnostics = queue_graph_id_diagnostics(items);
    let preliminary = queue_graph_diagnostics(&execution_mode, items, metadata, None, diagnostics.clone());
    if preliminary.has_errors() {
        return Err(QueueGraphValidationError::from_diagnostics(preliminary).into());
    }

    normalize_graph_dependencies(items, &mut diagnostics);
    if let Some(cycle) = queue_dependency_graph_cycle(items) {
        diagnostics.push(queue_graph_cycle_diagnostic(cycle));
        let report = queue_graph_diagnostics(&execution_mode, items, metadata, None, diagnostics);
        return Err(QueueGraphValidationError::from_diagnostics(report).into());
    }

    let wave_indices = queue_graph_wave_indices(items);
    diagnostics.extend(queue_graph_wave_conflicts(items, metadata, &wave_indices));
    Ok(queue_graph_diagnostics(
        &execution_mode,
        items,
        metadata,
        Some(&wave_indices),
        diagnostics,
    ))
}

fn queue_graph_id_diagnostics(items: &[state::QueueItemRow]) -> Vec<QueueGraphDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut by_id = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        if item.id.trim().is_empty() {
            diagnostics.push(queue_graph_diagnostic(
                QueueGraphDiagnosticKind::MissingId,
                QueueGraphDiagnosticSeverity::Error,
                "queue item id is empty",
                Some(item),
                None,
            ));
            continue;
        }
        if by_id.insert(item.id.as_str(), index).is_some() {
            diagnostics.push(queue_graph_diagnostic(
                QueueGraphDiagnosticKind::DuplicateId,
                QueueGraphDiagnosticSeverity::Error,
                format!("queue item id '{}' is duplicated", item.id),
                Some(item),
                None,
            ));
        }
    }
    diagnostics
}

fn normalize_graph_dependencies(
    items: &mut [state::QueueItemRow],
    diagnostics: &mut Vec<QueueGraphDiagnostic>,
) {
    let mut references = HashMap::new();
    for item in items.iter() {
        references.insert(item.id.clone(), item.id.clone());
    }
    for item in items.iter() {
        references
            .entry(item.slug.clone())
            .or_insert_with(|| item.id.clone());
    }
    for item in items.iter_mut() {
        let item_id = item.id.clone();
        let dependencies = std::mem::take(&mut item.depends_on);
        let mut seen = HashSet::new();
        item.depends_on = dependencies
            .into_iter()
            .filter_map(|dependency| {
                normalize_graph_dependency(&references, item, &item_id, &mut seen, dependency, diagnostics)
            })
            .collect();
    }
}

fn normalize_graph_dependency(
    references: &HashMap<String, String>,
    item: &state::QueueItemRow,
    item_id: &str,
    seen: &mut HashSet<String>,
    dependency: String,
    diagnostics: &mut Vec<QueueGraphDiagnostic>,
) -> Option<String> {
    if dependency.trim().is_empty() {
        diagnostics.push(queue_graph_dependency_diagnostic(
            QueueGraphDiagnosticKind::InvalidDependency,
            "queue dependency is empty",
            item,
            dependency,
            None,
        ));
        return None;
    }
    let Some(dependency_id) = references.get(&dependency).cloned() else {
        diagnostics.push(queue_graph_dependency_diagnostic(
            QueueGraphDiagnosticKind::MissingDependency,
            format!("queue dependency '{dependency}' does not match an item id or slug"),
            item,
            dependency,
            None,
        ));
        return None;
    };
    if dependency_id == item_id {
        diagnostics.push(queue_graph_dependency_diagnostic(
            QueueGraphDiagnosticKind::InvalidDependency,
            format!("queue item '{}' cannot depend on itself", item.id),
            item,
            dependency,
            Some(dependency_id),
        ));
        return None;
    }
    if !seen.insert(dependency_id.clone()) {
        diagnostics.push(queue_graph_dependency_diagnostic(
            QueueGraphDiagnosticKind::InvalidDependency,
            format!("queue dependency '{}' is duplicated for item '{}'", dependency, item.id),
            item,
            dependency,
            Some(dependency_id),
        ));
        return None;
    }
    Some(dependency_id)
}

fn queue_dependency_graph_cycle(items: &[state::QueueItemRow]) -> Option<Vec<String>> {
    let by_id = items
        .iter()
        .map(|item| (item.id.as_str(), item.depends_on.as_slice()))
        .collect::<HashMap<_, _>>();
    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    for item in items {
        if let Some(cycle) = queue_dependency_cycle_visit(&by_id, item.id.as_str(), &mut visited, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

fn queue_dependency_cycle_visit<'a>(
    by_id: &HashMap<&'a str, &'a [String]>,
    id: &'a str,
    visited: &mut HashSet<&'a str>,
    stack: &mut Vec<&'a str>,
) -> Option<Vec<String>> {
    if let Some(index) = stack.iter().position(|candidate| *candidate == id) {
        let mut cycle = stack[index..]
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        cycle.push(id.to_string());
        return Some(cycle);
    }
    if visited.contains(id) {
        return None;
    }
    stack.push(id);
    if let Some(dependencies) = by_id.get(id) {
        for dependency in *dependencies {
            if let Some(cycle) = queue_dependency_cycle_visit(by_id, dependency.as_str(), visited, stack) {
                return Some(cycle);
            }
        }
    }
    stack.pop();
    visited.insert(id);
    None
}

fn queue_graph_wave_indices(items: &[state::QueueItemRow]) -> HashMap<String, usize> {
    let by_id = items
        .iter()
        .map(|item| (item.id.as_str(), item.depends_on.as_slice()))
        .collect::<HashMap<_, _>>();
    let mut memo = HashMap::new();
    for item in items {
        let wave_index = queue_graph_wave_index(item.id.as_str(), &by_id, &mut memo);
        memo.insert(item.id.clone(), wave_index);
    }
    memo
}

fn queue_graph_wave_index<'a>(
    id: &'a str,
    by_id: &HashMap<&'a str, &'a [String]>,
    memo: &mut HashMap<String, usize>,
) -> usize {
    if let Some(wave_index) = memo.get(id) {
        return *wave_index;
    }
    let wave_index = by_id.get(id).map_or(0, |dependencies| {
            dependencies
                .iter()
                .map(|dependency| queue_graph_wave_index(dependency.as_str(), by_id, memo).saturating_add(1))
                .max()
                .unwrap_or(0)
        });
    memo.insert(id.to_string(), wave_index);
    wave_index
}

fn queue_graph_wave_conflicts(
    items: &[state::QueueItemRow],
    metadata: &[QueueGraphItemMetadata],
    wave_indices: &HashMap<String, usize>,
) -> Vec<QueueGraphDiagnostic> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let metadata = metadata.get(index)?;
            let requested_wave_index = metadata.wave_index?;
            let canonical_wave_index = *wave_indices.get(&item.id)?;
            if requested_wave_index == canonical_wave_index {
                return None;
            }
            let mut diagnostic = queue_graph_diagnostic(
                QueueGraphDiagnosticKind::WaveConflict,
                QueueGraphDiagnosticSeverity::Warning,
                format!(
                    "queue item '{}' was submitted in wave {}, canonical dependency wave is {}",
                    item.id, requested_wave_index, canonical_wave_index
                ),
                Some(item),
                None,
            );
            diagnostic.requested_wave_id.clone_from(&metadata.wave_id);
            diagnostic.requested_wave_index = Some(requested_wave_index);
            diagnostic.canonical_wave_index = Some(canonical_wave_index);
            Some(diagnostic)
        })
        .collect()
}

fn queue_graph_diagnostics(
    execution_mode: &state::QueueExecutionMode,
    items: &[state::QueueItemRow],
    metadata: &[QueueGraphItemMetadata],
    wave_indices: Option<&HashMap<String, usize>>,
    diagnostics: Vec<QueueGraphDiagnostic>,
) -> QueueGraphDiagnostics {
    let ok = diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity != QueueGraphDiagnosticSeverity::Error);
    QueueGraphDiagnostics {
        ok,
        execution_mode: execution_mode.as_str().to_string(),
        diagnostics,
        items: items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let metadata = metadata.get(index);
                QueueGraphItemDiagnostic {
                    id: item.id.clone(),
                    slug: item.slug.clone(),
                    position: item.position,
                    depends_on: item.depends_on.clone(),
                    requested_wave_id: metadata.and_then(|metadata| metadata.wave_id.clone()),
                    requested_wave_index: metadata.and_then(|metadata| metadata.wave_index),
                    canonical_wave_index: wave_indices.and_then(|indices| indices.get(&item.id).copied()),
                }
            })
            .collect(),
    }
}

fn queue_graph_diagnostic(
    kind: QueueGraphDiagnosticKind,
    severity: QueueGraphDiagnosticSeverity,
    message: impl Into<String>,
    item: Option<&state::QueueItemRow>,
    dependency_id: Option<String>,
) -> QueueGraphDiagnostic {
    QueueGraphDiagnostic {
        kind,
        severity,
        message: message.into(),
        item_id: item.map(|item| item.id.clone()).filter(|id| !id.is_empty()),
        item_slug: item.map(|item| item.slug.clone()).filter(|slug| !slug.is_empty()),
        item_position: item.map(|item| item.position),
        dependency: None,
        dependency_id,
        requested_wave_id: None,
        requested_wave_index: None,
        canonical_wave_index: None,
        cycle: Vec::new(),
    }
}

fn queue_graph_dependency_diagnostic(
    kind: QueueGraphDiagnosticKind,
    message: impl Into<String>,
    item: &state::QueueItemRow,
    dependency: String,
    dependency_id: Option<String>,
) -> QueueGraphDiagnostic {
    let mut diagnostic = queue_graph_diagnostic(
        kind,
        QueueGraphDiagnosticSeverity::Warning,
        message,
        Some(item),
        dependency_id,
    );
    diagnostic.dependency = Some(dependency);
    diagnostic
}

fn queue_graph_cycle_diagnostic(cycle: Vec<String>) -> QueueGraphDiagnostic {
    let message = if cycle.is_empty() {
        "queue dependency graph contains a cycle".to_string()
    } else {
        format!("queue dependency graph contains a cycle: {}", cycle.join(" -> "))
    };
    QueueGraphDiagnostic {
        kind: QueueGraphDiagnosticKind::Cycle,
        severity: QueueGraphDiagnosticSeverity::Error,
        message,
        item_id: cycle.first().cloned(),
        item_slug: None,
        item_position: None,
        dependency: None,
        dependency_id: None,
        requested_wave_id: None,
        requested_wave_index: None,
        canonical_wave_index: None,
        cycle,
    }
}

fn clean_queue_graph_wave_id(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
