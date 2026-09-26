use morrows_core::{Id, MemoryEntry};
use morrows_store::Store;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{fs, process::Command, sync::Mutex, time::timeout};

const DEFAULT_TOP_K: usize = 8;
const MAX_TOP_K: usize = 20;
const MAX_QUERY_BYTES: usize = 4096;
const MAX_TERMS: usize = 16;

#[derive(Clone)]
pub(crate) struct MemorySearch {
    cfg: Arc<MemorySearchConfig>,
    operation_lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct MemorySearchConfig {
    rg: PathBuf,
    projection_root: PathBuf,
    timeout: Duration,
}

#[derive(Default)]
struct HitScore {
    matched_terms: Vec<String>,
    match_count: usize,
    exact_phrase: bool,
}

impl MemorySearch {
    pub(crate) fn managed() -> Option<Self> {
        let root = env::var_os("MORROWS_ROOT")
            .map(PathBuf::from)
            .or_else(|| env::current_dir().ok())?;
        let rg = env::var_os("MORROWS_RG_BIN")
            .map(PathBuf::from)
            .or_else(|| find_in_path("rg"))?;
        Some(Self::from_config(MemorySearchConfig {
            rg,
            projection_root: root.join("data/memory-grep/projection"),
            timeout: Duration::from_secs(15),
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test(rg: PathBuf, projection_root: PathBuf) -> Self {
        Self::from_config(MemorySearchConfig {
            rg,
            projection_root,
            timeout: Duration::from_secs(5),
        })
    }

    fn from_config(cfg: MemorySearchConfig) -> Self {
        Self {
            cfg: Arc::new(cfg),
            operation_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) async fn search_project(
        &self,
        store: &Store,
        project_id: Id,
        query: &str,
        top_k: usize,
    ) -> Result<Value, String> {
        let query = query.trim();
        if query.is_empty() {
            return Err("memory search query cannot be empty".into());
        }
        if query.len() > MAX_QUERY_BYTES {
            return Err(format!(
                "memory search query exceeds {MAX_QUERY_BYTES} bytes"
            ));
        }
        if !(1..=MAX_TOP_K).contains(&top_k) {
            return Err(format!("memory search top_k must be 1..={MAX_TOP_K}"));
        }

        let terms = search_terms(query);
        if terms.is_empty() {
            return Ok(json!({
                "available": true,
                "engine": "ripgrep",
                "query": query,
                "terms": [],
                "results": [],
                "authoritative_source": "morrows_memory_entry",
                "index_role": "none",
            }));
        }

        let _operation = self.operation_lock.lock().await;
        let current = self.sync_projection(store, project_id).await?;
        if current.is_empty() {
            return Ok(json!({
                "available": true,
                "engine": "ripgrep",
                "query": query,
                "terms": terms.iter().map(|(term, _)| term).collect::<Vec<_>>(),
                "results": [],
                "indexed_memory_count": 0,
                "authoritative_source": "morrows_memory_entry",
                "index_role": "none",
            }));
        }

        let project_root = self.cfg.projection_root.join(project_id.to_string());
        let mut scores: HashMap<Id, HitScore> = HashMap::new();
        for (term, exact_phrase) in &terms {
            let matches = self.grep_term(&project_root, term).await?;
            for (id, count) in matches {
                if !current.contains_key(&id) {
                    continue;
                }
                let score = scores.entry(id).or_default();
                score.matched_terms.push(term.clone());
                score.match_count += count;
                score.exact_phrase |= *exact_phrase;
            }
        }

        let mut ranked: Vec<_> = scores.into_iter().collect();
        ranked.sort_by(|(left_id, left), (right_id, right)| {
            right
                .exact_phrase
                .cmp(&left.exact_phrase)
                .then_with(|| right.matched_terms.len().cmp(&left.matched_terms.len()))
                .then_with(|| right.match_count.cmp(&left.match_count))
                .then_with(|| {
                    let left_created = current.get(left_id).map(|m| m.created_at);
                    let right_created = current.get(right_id).map(|m| m.created_at);
                    right_created.cmp(&left_created)
                })
        });

        let results: Vec<_> = ranked
            .into_iter()
            .take(top_k)
            .filter_map(|(id, score)| {
                current.get(&id).map(|memory| {
                    json!({
                        "exact_phrase": score.exact_phrase,
                        "matched_terms": score.matched_terms,
                        "match_count": score.match_count,
                        "memory": memory,
                    })
                })
            })
            .collect();

        Ok(json!({
            "available": true,
            "engine": "ripgrep",
            "query": query,
            "terms": terms.iter().map(|(term, _)| term).collect::<Vec<_>>(),
            "indexed_memory_count": current.len(),
            "results": results,
            "authoritative_source": "morrows_memory_entry",
            "index_role": "none",
            "ranking": "exact_phrase_then_unique_terms_then_match_count_then_recency",
        }))
    }

    pub(crate) async fn default_project_retrieval(
        &self,
        store: &Store,
        project_id: Id,
        query: &str,
    ) -> Value {
        match self
            .search_project(store, project_id, query, DEFAULT_TOP_K)
            .await
        {
            Ok(value) => value,
            Err(error) => json!({
                "available": false,
                "engine": "ripgrep",
                "error": error,
                "fallback": "task_context.memory",
            }),
        }
    }

    async fn grep_term(&self, root: &Path, term: &str) -> Result<HashMap<Id, usize>, String> {
        let mut command = Command::new(&self.cfg.rg);
        command
            .arg("--json")
            .arg("--ignore-case")
            .arg("--fixed-strings")
            .arg("--glob")
            .arg("*.md")
            .arg("--")
            .arg(term)
            .arg(root)
            .kill_on_drop(true);
        let output = timeout(self.cfg.timeout, command.output())
            .await
            .map_err(|_| format!("ripgrep timed out while searching {term:?}"))?
            .map_err(|e| format!("start ripgrep: {e}"))?;
        if !matches!(output.status.code(), Some(0 | 1)) {
            return Err(format!(
                "ripgrep failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        let mut counts = HashMap::new();
        for line in output.stdout.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_slice(line).map_err(|e| format!("decode ripgrep JSON: {e}"))?;
            if value["type"] != "match" {
                continue;
            }
            let Some(path) = value["data"]["path"]["text"].as_str() else {
                continue;
            };
            let Some(id) = memory_id_from_source(path) else {
                continue;
            };
            let count = value["data"]["submatches"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or(1)
                .max(1);
            *counts.entry(id).or_insert(0) += count;
        }
        Ok(counts)
    }

    async fn sync_projection(
        &self,
        store: &Store,
        project_id: Id,
    ) -> Result<HashMap<Id, MemoryEntry>, String> {
        let project_root = self.cfg.projection_root.join(project_id.to_string());
        fs::create_dir_all(&project_root)
            .await
            .map_err(|e| format!("create project memory projection: {e}"))?;

        let mut offset = 0;
        let mut entries = HashMap::new();
        loop {
            let page = store
                .context_memories_page(None, Some(project_id), None, false, 100, offset)
                .await
                .map_err(|e| e.to_string())?;
            for entry in page.items {
                if entry.visibility == "shared"
                    && (entry.scope_type == "organization"
                        || (entry.scope_type == "project" && entry.project_id == Some(project_id)))
                {
                    entries.insert(entry.id, entry);
                }
            }
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }

        let expected: HashSet<_> = entries.keys().map(|id| format!("{id}.md")).collect();
        for entry in entries.values() {
            let path = project_root.join(format!("{}.md", entry.id));
            let tmp = project_root.join(format!(".{}.md.tmp", entry.id));
            fs::write(&tmp, render_memory(entry))
                .await
                .map_err(|e| format!("write memory projection: {e}"))?;
            fs::rename(&tmp, &path)
                .await
                .map_err(|e| format!("publish memory projection: {e}"))?;
        }
        let mut dir = fs::read_dir(&project_root)
            .await
            .map_err(|e| format!("read memory projection directory: {e}"))?;
        while let Some(item) = dir
            .next_entry()
            .await
            .map_err(|e| format!("read memory projection entry: {e}"))?
        {
            let name = item.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !expected.contains(&name) {
                fs::remove_file(item.path())
                    .await
                    .map_err(|e| format!("remove stale memory projection: {e}"))?;
            } else if name.ends_with(".tmp") {
                let _ = fs::remove_file(item.path()).await;
            }
        }
        Ok(entries)
    }
}

fn search_terms(query: &str) -> Vec<(String, bool)> {
    const STOP: &[&str] = &[
        "task",
        "description",
        "goal",
        "current",
        "summary",
        "the",
        "and",
        "for",
        "with",
        "from",
        "this",
        "that",
        "what",
        "when",
        "where",
        "how",
        "why",
        "into",
        "about",
    ];
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let trimmed = query.trim();
    if trimmed.len() >= 4 && trimmed.len() <= 160 && !trimmed.contains('\n') {
        seen.insert(trimmed.to_lowercase());
        terms.push((trimmed.to_string(), true));
    }

    let mut ascii = String::new();
    let mut cjk = String::new();
    let flush_ascii =
        |buf: &mut String, terms: &mut Vec<(String, bool)>, seen: &mut HashSet<String>| {
            if buf.len() >= 2 {
                let lower = buf.to_lowercase();
                if !STOP.contains(&lower.as_str()) && seen.insert(lower) {
                    terms.push((buf.clone(), false));
                }
            }
            buf.clear();
        };
    let flush_cjk =
        |buf: &mut String, terms: &mut Vec<(String, bool)>, seen: &mut HashSet<String>| {
            let chars: Vec<char> = buf.chars().collect();
            if chars.len() >= 2 {
                let full: String = chars.iter().collect();
                if chars.len() <= 8 && seen.insert(full.clone()) {
                    terms.push((full, false));
                }
                if chars.len() > 2 {
                    for pair in chars.windows(2) {
                        let term: String = pair.iter().collect();
                        if seen.insert(term.clone()) {
                            terms.push((term, false));
                        }
                        if terms.len() >= MAX_TERMS {
                            break;
                        }
                    }
                }
            }
            buf.clear();
        };

    for ch in query.chars() {
        if is_cjk(ch) {
            flush_ascii(&mut ascii, &mut terms, &mut seen);
            cjk.push(ch);
        } else if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
            flush_cjk(&mut cjk, &mut terms, &mut seen);
            ascii.push(ch);
        } else {
            flush_ascii(&mut ascii, &mut terms, &mut seen);
            flush_cjk(&mut cjk, &mut terms, &mut seen);
        }
        if terms.len() >= MAX_TERMS {
            break;
        }
    }
    if terms.len() < MAX_TERMS {
        flush_ascii(&mut ascii, &mut terms, &mut seen);
        flush_cjk(&mut cjk, &mut terms, &mut seen);
    }
    terms.truncate(MAX_TERMS);
    terms
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF
    )
}

fn render_memory(entry: &MemoryEntry) -> String {
    let content =
        serde_json::to_string_pretty(&entry.content).unwrap_or_else(|_| entry.content.to_string());
    format!(
        "# {}\n\nScope: {}\nSource kind: {}\nCreated: {}\n\n## Content\n\n{}\n",
        entry.title, entry.scope_type, entry.source_kind, entry.created_at, content
    )
}

fn memory_id_from_source(source: &str) -> Option<Id> {
    let stem = Path::new(source).file_stem()?.to_str()?;
    Id::parse_str(stem).ok()
}

fn find_in_path(program: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_terms_cover_exact_ascii_and_chinese_bigrams() {
        let english = search_terms("Why memory_disposition failed in completion");
        assert!(english.iter().any(|(term, _)| term == "memory_disposition"));
        assert!(english.iter().any(|(term, _)| term == "completion"));
        let chinese = search_terms("为什么不用向量数据库");
        assert!(chinese.iter().any(|(term, _)| term == "向量"));
        assert!(
            chinese
                .iter()
                .any(|(term, _)| term == "数据库" || term == "数据")
        );
    }

    #[test]
    fn source_file_maps_back_to_memory_id() {
        let id = Id::new_v4();
        assert_eq!(
            memory_id_from_source(&format!("/tmp/project/{id}.md")),
            Some(id)
        );
        assert_eq!(memory_id_from_source("/tmp/project/not-an-id.md"), None);
    }
}
