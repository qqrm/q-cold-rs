const PROOF_RUN_INDEX: &str = "compat/evidence/proof-runs.tsv";
const PROOF_RUN_INDEX_LIMIT: usize = 20;
const PROOF_RUN_SUMMARY_ROOT: &str = ".task/logs/compat";
const PROOF_RUN_INDEX_HEADER: [&str; 17] = [
    "task_sequence",
    "task_id",
    "task_name",
    "task_head",
    "base_head",
    "suite",
    "profile",
    "baseline_source",
    "baseline_ref",
    "selected",
    "matched",
    "regressions",
    "timeouts",
    "executed",
    "reused_matched",
    "status",
    "failure_rows",
];

#[derive(Clone)]
struct ProofRunRow {
    values: Vec<String>,
}

struct ProofRunSummary {
    suite: String,
    fields: std::collections::BTreeMap<String, String>,
    directory: PathBuf,
}

fn update_proof_run_index(task: &TaskEnv) -> Result<()> {
    let summaries = proof_run_summaries(task)?;
    let index = task.task_worktree.join(PROOF_RUN_INDEX);
    if summaries.is_empty() && !index.is_file() {
        return Ok(());
    }

    let mut rows = read_proof_run_index(&index)?;
    let generated = summaries
        .into_iter()
        .map(|summary| proof_run_row(task, &summary))
        .collect::<Vec<_>>();
    if !generated.is_empty() {
        rows.retain(|row| !generated.iter().any(|current| current.same_identity(row)));
        rows.extend(generated);
    }
    trim_proof_run_rows(&mut rows);
    write_proof_run_index(&index, &rows)
}

fn proof_run_summaries(task: &TaskEnv) -> Result<Vec<ProofRunSummary>> {
    let root = task.task_worktree.join(PROOF_RUN_SUMMARY_ROOT);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    collect_summary_paths(&root, &mut paths)?;
    paths
        .into_iter()
        .filter_map(|path| proof_run_summary(&root, &path).transpose())
        .collect()
}

fn collect_summary_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_summary_paths(&path, paths)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("summary.tsv") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(())
}

fn proof_run_summary(root: &Path, path: &Path) -> Result<Option<ProofRunSummary>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut fields = parse_summary_fields(&content);
    merge_summary_env(&mut fields, &path.with_file_name("proof-run.env"))?;
    if !proof_run_active(&fields) {
        return Ok(None);
    }
    let suite = fields
        .get("suite")
        .cloned()
        .unwrap_or_else(|| suite_from_summary_path(root, path));
    Ok(Some(ProofRunSummary {
        suite,
        fields,
        directory: path.parent().unwrap_or(root).to_path_buf(),
    }))
}

fn merge_summary_env(
    fields: &mut std::collections::BTreeMap<String, String>,
    path: &Path,
) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    for line in content.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.insert(normalize_summary_key(key), unquote(value));
        }
    }
    Ok(())
}

fn parse_summary_fields(content: &str) -> std::collections::BTreeMap<String, String> {
    let mut fields = std::collections::BTreeMap::new();
    let mut table_header: Option<Vec<String>> = None;
    for line in content.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with('#') {
            continue;
        }
        if parse_key_value_cells(line, &mut fields) {
            continue;
        }
        let cells = split_tsv_line(line);
        if let Some(header) = table_header.take() {
            merge_table_row(&mut fields, &header, &cells);
            break;
        }
        table_header = Some(cells.into_iter().map(|cell| normalize_summary_key(&cell)).collect());
    }
    fields
}

fn parse_key_value_cells(
    line: &str,
    fields: &mut std::collections::BTreeMap<String, String>,
) -> bool {
    let mut parsed = false;
    for cell in line.split_whitespace() {
        if let Some((key, value)) = cell.split_once('=') {
            fields.insert(normalize_summary_key(key), unquote(value));
            parsed = true;
        }
    }
    parsed
}

fn merge_table_row(
    fields: &mut std::collections::BTreeMap<String, String>,
    header: &[String],
    row: &[String],
) {
    for (key, value) in header.iter().zip(row) {
        fields.insert(key.clone(), value.clone());
    }
}

fn split_tsv_line(line: &str) -> Vec<String> {
    line.split('\t').map(|cell| cell.trim().to_string()).collect()
}

fn normalize_summary_key(key: &str) -> String {
    key.trim()
        .trim_start_matches("PROOF_RUN_")
        .to_ascii_lowercase()
}

fn proof_run_active(fields: &std::collections::BTreeMap<String, String>) -> bool {
    ["selected", "compat_rows", "rows", "matched", "regressions", "timeouts"]
        .iter()
        .any(|key| numeric_field(fields, key) > 0)
}

fn proof_run_row(task: &TaskEnv, summary: &ProofRunSummary) -> ProofRunRow {
    let selected = field_any(&summary.fields, &["selected", "compat_rows", "rows"]);
    let matched = field_any(&summary.fields, &["matched", "passed"]);
    let regressions = field(&summary.fields, "regressions");
    let timeouts = field(&summary.fields, "timeouts");
    let status = summary
        .fields
        .get("status")
        .cloned()
        .unwrap_or_else(|| derived_proof_status(&selected, &matched, &regressions, &timeouts));
    let failure_rows = summary
        .fields
        .get("failure_rows")
        .cloned()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| collect_failure_rows(&summary.directory));
    ProofRunRow {
        values: vec![
            task_sequence_for_index(task),
            task.task_id.clone(),
            task.task_name.clone(),
            task.task_head.clone(),
            task.base_head.clone(),
            summary.suite.clone(),
            fallback_field(&summary.fields, "profile", &task.task_profile),
            field(&summary.fields, "baseline_source"),
            baseline_ref(&summary.fields),
            selected,
            matched,
            regressions,
            timeouts,
            field(&summary.fields, "executed"),
            field(&summary.fields, "reused_matched"),
            status,
            failure_rows,
        ],
    }
}

fn task_sequence_for_index(task: &TaskEnv) -> String {
    if !task.task_sequence.is_empty() {
        task.task_sequence.clone()
    } else if task.task_execution_anchor.chars().all(|ch| ch.is_ascii_digit()) {
        task.task_execution_anchor.clone()
    } else {
        String::new()
    }
}

fn baseline_ref(fields: &std::collections::BTreeMap<String, String>) -> String {
    [
        "baseline_ref",
        "baseline_digest",
        "image_digest",
        "baseline_path",
    ]
    .iter()
    .find_map(|key| fields.get(*key).cloned().filter(|value| !value.is_empty()))
    .unwrap_or_default()
}

fn field(fields: &std::collections::BTreeMap<String, String>, key: &str) -> String {
    fields.get(key).cloned().unwrap_or_default()
}

fn field_any(fields: &std::collections::BTreeMap<String, String>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| fields.get(*key).cloned().filter(|value| !value.is_empty()))
        .unwrap_or_default()
}

fn fallback_field(
    fields: &std::collections::BTreeMap<String, String>,
    key: &str,
    fallback: &str,
) -> String {
    fields
        .get(key)
        .cloned()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn numeric_field(fields: &std::collections::BTreeMap<String, String>, key: &str) -> u64 {
    fields
        .get(key)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn derived_proof_status(selected: &str, matched: &str, regressions: &str, timeouts: &str) -> String {
    let selected = selected.parse::<u64>().unwrap_or(0);
    let matched = matched.parse::<u64>().unwrap_or(0);
    let regressions = regressions.parse::<u64>().unwrap_or(0);
    let timeouts = timeouts.parse::<u64>().unwrap_or(0);
    if selected > 0 && matched == selected && regressions == 0 && timeouts == 0 {
        "pass".to_string()
    } else {
        "fail".to_string()
    }
}

fn collect_failure_rows(directory: &Path) -> String {
    let mut rows = BTreeSet::new();
    for name in ["regressions.tsv", "timeouts.tsv"] {
        collect_failure_rows_from_file(&directory.join(name), &mut rows);
    }
    let total = rows.len();
    let mut selected = rows.into_iter().take(4).collect::<Vec<_>>();
    if total > selected.len() {
        selected.push(format!("+{}_more", total - selected.len()));
    }
    selected.join(";")
}

fn collect_failure_rows_from_file(path: &Path, rows: &mut BTreeSet<String>) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    for line in content.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(cell) = line.split('\t').next().map(str::trim) else {
            continue;
        };
        if matches!(cell, "test" | "name" | "row" | "failure_row") {
            continue;
        }
        rows.insert(cell.to_string());
    }
}

fn suite_from_summary_path(root: &Path, path: &Path) -> String {
    path.parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .map(path_to_suite)
        .filter(|suite| !suite.is_empty())
        .unwrap_or_else(|| "compat".to_string())
}

fn path_to_suite(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn read_proof_run_index(path: &Path) -> Result<Vec<ProofRunRow>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut lines = content.lines().filter(|line| !line.trim().is_empty());
    let header = lines
        .next()
        .map(split_tsv_line)
        .context("proof run index is missing a header")?;
    if header != PROOF_RUN_INDEX_HEADER {
        bail!("proof run index header drifted: {}", path.display());
    }
    lines.map(parse_proof_run_row).collect()
}

fn parse_proof_run_row(line: &str) -> Result<ProofRunRow> {
    let values = split_tsv_line(line);
    if values.len() != PROOF_RUN_INDEX_HEADER.len() {
        bail!("proof run index row has {} fields", values.len());
    }
    if values.iter().any(|value| contains_bundle_reference(value)) {
        bail!("proof run index row contains a bundle reference");
    }
    Ok(ProofRunRow { values })
}

fn contains_bundle_reference(value: &str) -> bool {
    value.contains("bundles/")
        || Path::new(value)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn write_proof_run_index(path: &Path, rows: &[ProofRunRow]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = PROOF_RUN_INDEX_HEADER.join("\t");
    output.push('\n');
    for row in rows {
        output.push_str(&row.to_line()?);
        output.push('\n');
    }
    fs::write(path, output).with_context(|| format!("failed to write {}", path.display()))
}

fn trim_proof_run_rows(rows: &mut Vec<ProofRunRow>) {
    if rows.len() > PROOF_RUN_INDEX_LIMIT {
        let keep_from = rows.len() - PROOF_RUN_INDEX_LIMIT;
        rows.drain(..keep_from);
    }
}

impl ProofRunRow {
    fn same_identity(&self, other: &Self) -> bool {
        [0, 1, 3, 5]
            .iter()
            .all(|index| self.values.get(*index) == other.values.get(*index))
    }

    fn to_line(&self) -> Result<String> {
        if self.values.iter().any(|value| contains_bundle_reference(value)) {
            bail!("proof run index row contains a bundle reference");
        }
        Ok(self
            .values
            .iter()
            .map(|value| tsv_clean(value))
            .collect::<Vec<_>>()
            .join("\t"))
    }
}

fn tsv_clean(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}
