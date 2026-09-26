use morrows_core::{Id, MemoryEntry};
use morrows_store::Store;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{fs, process::Command, sync::Mutex, time::timeout};

const DEFAULT_COLLECTION: &str = "morrows_project_memory";
const DEFAULT_TOP_K: usize = 8;
const MAX_TOP_K: usize = 20;
const MAX_QUERY_BYTES: usize = 4096;

#[derive(Clone)]
pub(crate) struct MemorySearch {
    cfg: Arc<MemorySearchConfig>,
    lock: Arc<Mutex<()>>,
}

#[derive(Clone)]
struct MemorySearchConfig {
    python: PathBuf,
    bridge: PathBuf,
    projection_root: PathBuf,
    milvus_uri: PathBuf,
    collection: String,
    timeout: Duration,
}

#[derive(Debug, Deserialize)]
struct BridgeResult {
    engine: String,
    version: String,
    provider: String,
    model: String,
    indexed_chunks: usize,
    results: Vec<BridgeHit>,
}

#[derive(Debug, Deserialize)]
struct BridgeHit {
    source: String,
    #[serde(default)]
    heading: String,
    #[serde(default)]
    score: f64,
}

impl MemorySearch {
    pub(crate) fn managed() -> Option<Self> {
        let root = env::var_os("MORROWS_ROOT")
            .map(PathBuf::from)
            .or_else(|| env::current_dir().ok())?;
        let home = env::var_os("HOME").map(PathBuf::from)?;
        let python = env::var_os("MORROWS_MEMSEARCH_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share/morrows-memsearch/venv/bin/python"));
        let bridge = root.join("scripts/memory_search_bridge.py");
        if !python.is_file() || !bridge.is_file() {
            return None;
        }
        Some(Self::from_config(MemorySearchConfig {
            python,
            bridge,
            projection_root: root.join("data/memory-search/projection"),
            milvus_uri: root.join("data/memory-search/milvus.db"),
            collection: DEFAULT_COLLECTION.into(),
            timeout: Duration::from_secs(180),
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        python: PathBuf,
        bridge: PathBuf,
        projection_root: PathBuf,
        milvus_uri: PathBuf,
    ) -> Self {
        Self::from_config(MemorySearchConfig {
            python,
            bridge,
            projection_root,
            milvus_uri,
            collection: "morrows_test_memory".into(),
            timeout: Duration::from_secs(15),
        })
    }

    fn from_config(cfg: MemorySearchConfig) -> Self {
        Self {
            cfg: Arc::new(cfg),
            lock: Arc::new(Mutex::new(())),
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
        let _guard = self.lock.lock().await;
        let current = self.sync_projection(store, project_id).await?;
        if current.is_empty() {
            return Ok(json!({
                "available": true,
                "engine": "memsearch",
                "query": query,
                "results": [],
                "indexed_memory_count": 0,
            }));
        }

        let project_root = self.cfg.projection_root.join(project_id.to_string());
        if let Some(parent) = self.cfg.milvus_uri.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("create memory-search state directory: {e}"))?;
        }
        let fetch_k = (top_k * 3).min(60);
        let mut command = Command::new(&self.cfg.python);
        command
            .arg(&self.cfg.bridge)
            .arg("index-search")
            .arg("--root")
            .arg(&project_root)
            .arg("--milvus-uri")
            .arg(&self.cfg.milvus_uri)
            .arg("--collection")
            .arg(&self.cfg.collection)
            .arg("--query")
            .arg(query)
            .arg("--top-k")
            .arg(fetch_k.to_string())
            .kill_on_drop(true);
        let output = timeout(self.cfg.timeout, command.output())
            .await
            .map_err(|_| "MemSearch bridge timed out".to_string())?
            .map_err(|e| format!("start MemSearch bridge: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "MemSearch bridge failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let bridge: BridgeResult = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("decode MemSearch bridge output: {e}"))?;

        let mut seen = HashSet::new();
        let mut ranked = Vec::new();
        for hit in bridge.results {
            let Some(id) = memory_id_from_source(&hit.source) else {
                continue;
            };
            let Some(memory) = current.get(&id) else {
                // The vector index is a shadow only. Never return a stale hit
                // that is absent from the current authoritative projection.
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            ranked.push(json!({
                "score": hit.score,
                "matched_heading": hit.heading,
                "memory": memory,
            }));
            if ranked.len() == top_k {
                break;
            }
        }

        Ok(json!({
            "available": true,
            "engine": bridge.engine,
            "engine_version": bridge.version,
            "provider": bridge.provider,
            "model": bridge.model,
            "query": query,
            "indexed_chunks": bridge.indexed_chunks,
            "indexed_memory_count": current.len(),
            "results": ranked,
            "authoritative_source": "morrows_memory_entry",
            "index_role": "rebuildable_shadow",
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
                "engine": "memsearch",
                "error": error,
                "fallback": "task_context.memory",
            }),
        }
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
            // agent_id=None deliberately excludes private Agent memory. task_id=None
            // keeps this index to shared organization + project knowledge only.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shadow_hits_are_filtered_through_current_authoritative_memory() {
        use morrows_core::{CreateMemoryEntry, CreateProject};
        use std::fs as stdfs;

        let base = std::env::temp_dir().join(format!("morrows-memory-search-{}", Id::new_v4()));
        stdfs::create_dir_all(&base).unwrap();
        let bridge = base.join("fake_bridge.py");
        stdfs::write(
            &bridge,
            r#"import argparse,json
from pathlib import Path
p=argparse.ArgumentParser()
p.add_argument("command")
p.add_argument("--root")
p.add_argument("--milvus-uri")
p.add_argument("--collection")
p.add_argument("--query")
p.add_argument("--top-k")
a=p.parse_args()
files=sorted(Path(a.root).glob("*.md"))
stale=Path(a.root)/"00000000-0000-0000-0000-000000000001.md"
print(json.dumps({
  "engine":"fake-memsearch","version":"test","provider":"onnx","model":"fake",
  "indexed_chunks":2,
  "results":[
    {"source":str(stale),"heading":"stale","score":1.0},
    {"source":str(files[0]),"heading":"current","score":0.9},
    {"source":str(files[0]),"heading":"duplicate","score":0.8}
  ]
}))
"#,
        )
        .unwrap();

        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Search".into(),
                description: "Search test".into(),
            })
            .await
            .unwrap();
        let memory = store
            .create_memory_entry(CreateMemoryEntry {
                scope_type: "project".into(),
                project_id: Some(project.id),
                agent_instance_id: None,
                task_id: None,
                title: "Protocol".into(),
                content: json!({"sentinel":"AUTHORITATIVE-ALPHA-17"}),
                source_kind: "test".into(),
                source_ref: None,
                visibility: "shared".into(),
                supersedes_memory_id: None,
            })
            .await
            .unwrap();
        let search = MemorySearch::for_test(
            PathBuf::from("python3"),
            bridge,
            base.join("projection"),
            base.join("milvus.db"),
        );
        let result = search
            .search_project(&store, project.id, "alpha protocol", 5)
            .await
            .unwrap();
        let hits = result["results"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["memory"]["id"], json!(memory.id));
        assert_eq!(
            hits[0]["memory"]["content"]["sentinel"],
            "AUTHORITATIVE-ALPHA-17"
        );
        assert_eq!(result["index_role"], "rebuildable_shadow");
        let _ = stdfs::remove_dir_all(base);
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
