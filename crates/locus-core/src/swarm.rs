//! Immutable, operator-supplied tool grants for a swarm graph.
//!
//! A graph node's id labels an agent; it is never authentication. Callers must
//! authenticate the tenant token first and use its verified grant id for the
//! lookup. A missing node has no authority, including no control-tool fallback.
//! This manifest only narrows existing session and policy authority.

use crate::{is_safe_mcp_grant_id, LocusError, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_AGENTS: usize = 1024;
const MAX_TOOLS_PER_AGENT: usize = 1024;
const MAX_AGENT_ID_BYTES: usize = 128;
const MAX_TOOL_NAME_BYTES: usize = 256;

/// Validated graph snapshot. Reload requires restarting the broker: untrusted
/// requests cannot replace this graph or widen an agent's grant in flight.
#[derive(Debug, Clone)]
pub struct SwarmGraph {
    nodes: Vec<SwarmAgent>,
}

/// A graph node bound to one existing, operator-minted MCP tenant grant.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SwarmAgent {
    pub id: String,
    pub grant_id: String,
    /// Exact MCP tool names. Globs, prefixes and implicit control tools are
    /// deliberately unsupported; an empty list grants nothing.
    pub tools: Vec<String>,
}

impl SwarmAgent {
    pub fn allows(&self, tool: &str) -> bool {
        self.tools.iter().any(|allowed| allowed == tool)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    nodes: Vec<SwarmAgent>,
}

impl SwarmGraph {
    /// Number of graph nodes, safe to expose in the broker's launch audit.
    pub fn agent_count(&self) -> usize {
        self.nodes.len()
    }

    /// Inspect configured grants for startup preflight without granting the
    /// caller any way to mutate the loaded authorization snapshot.
    pub fn agents(&self) -> impl Iterator<Item = &SwarmAgent> {
        self.nodes.iter()
    }

    /// Read at most the manifest limit plus one byte. Errors never include the
    /// path, file contents, or parser diagnostics, which can contain secrets.
    pub fn load(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A mistakenly supplied FIFO must not hang broker startup before
            // the regular-file check below can reject it.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options
            .open(path)
            .map_err(|_| invalid("swarm_manifest_unreadable"))?;
        if !file
            .metadata()
            .map_err(|_| invalid("swarm_manifest_unreadable"))?
            .is_file()
        {
            return Err(invalid("swarm_manifest_unreadable"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_MANIFEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("swarm_manifest_unreadable"))?;
        Self::from_slice(&bytes)
    }

    /// Parse a strict JSON manifest. Duplicate identities are refused rather
    /// than choosing a first/last grant; a malformed graph fails as a whole.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(invalid("swarm_manifest_too_large"));
        }
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|_| invalid("swarm_manifest_invalid"))?;
        if manifest.version != 1 {
            return Err(invalid("swarm_manifest_version_unsupported"));
        }
        if manifest.nodes.len() > MAX_AGENTS {
            return Err(invalid("swarm_manifest_too_many_agents"));
        }
        let mut ids = HashSet::new();
        let mut grants = HashSet::new();
        for node in &manifest.nodes {
            if !valid_agent_id(&node.id) {
                return Err(invalid("swarm_manifest_agent_id_invalid"));
            }
            if !is_safe_mcp_grant_id(&node.grant_id) {
                return Err(invalid("swarm_manifest_grant_id_invalid"));
            }
            if !ids.insert(&node.id) || !grants.insert(&node.grant_id) {
                return Err(invalid("swarm_manifest_duplicate_identity"));
            }
            if node.tools.len() > MAX_TOOLS_PER_AGENT {
                return Err(invalid("swarm_manifest_too_many_tools"));
            }
            let mut tools = HashSet::new();
            for tool in &node.tools {
                if !valid_tool_name(tool) {
                    return Err(invalid("swarm_manifest_tool_name_invalid"));
                }
                if !tools.insert(tool) {
                    return Err(invalid("swarm_manifest_duplicate_tool"));
                }
            }
        }
        Ok(Self {
            nodes: manifest.nodes,
        })
    }

    /// Resolve ONLY a cryptographically authenticated grant id. Never call
    /// this with a self-asserted agent id or a token's unverified public prefix.
    pub fn agent_for_grant(&self, grant_id: &str) -> Option<&SwarmAgent> {
        self.nodes.iter().find(|node| node.grant_id == grant_id)
    }
}

fn invalid(code: &'static str) -> LocusError {
    LocusError::msg(code)
}

fn valid_agent_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_AGENT_ID_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'/'))
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn manifest() -> Value {
        json!({
            "version": 1,
            "nodes": [{
                "id": "graph/research:1",
                "grant_id": "0123456789abcdef",
                "tools": ["github.scope", "locus_whoami"]
            }, {
                "id": "writer",
                "grant_id": "fedcba9876543210",
                "tools": []
            }]
        })
    }

    fn parse(value: &Value) -> Result<SwarmGraph> {
        SwarmGraph::from_slice(&serde_json::to_vec(value).unwrap())
    }

    fn error(value: &Value) -> String {
        parse(value).unwrap_err().to_string()
    }

    #[test]
    fn authenticated_grant_resolves_exact_graph_node_and_exact_tool_names() {
        let graph = parse(&manifest()).unwrap();
        assert_eq!(graph.agent_count(), 2);
        assert_eq!(graph.agents().count(), 2);
        let agent = graph.agent_for_grant("0123456789abcdef").unwrap();
        assert_eq!(agent.id, "graph/research:1");
        assert!(agent.allows("github.scope"));
        assert!(agent.allows("locus_whoami"));
        for denied in [
            "github",
            "github.*",
            "github.scope.extra",
            "GitHub.scope",
            "locus_status",
        ] {
            assert!(!agent.allows(denied));
        }
        assert!(graph.agent_for_grant("graph/research:1").is_none());
        assert!(graph.agent_for_grant("unknown").is_none());
        assert!(graph.agent_for_grant("").is_none());
        assert!(!graph
            .agent_for_grant("fedcba9876543210")
            .unwrap()
            .allows("locus_whoami"));
    }

    #[test]
    fn empty_graph_and_empty_tool_grants_are_valid_and_deny_everything() {
        let graph = parse(&json!({"version": 1, "nodes": []})).unwrap();
        assert_eq!(graph.agent_count(), 0);
        assert!(graph.agent_for_grant("0123456789abcdef").is_none());
        let graph = parse(&manifest()).unwrap();
        assert!(graph
            .agent_for_grant("fedcba9876543210")
            .unwrap()
            .tools
            .is_empty());
    }

    #[test]
    fn rejects_duplicate_node_ids_grant_ids_and_tools() {
        let mut value = manifest();
        value["nodes"][1]["id"] = value["nodes"][0]["id"].clone();
        assert_eq!(error(&value), "swarm_manifest_duplicate_identity");
        let mut value = manifest();
        value["nodes"][1]["grant_id"] = value["nodes"][0]["grant_id"].clone();
        assert_eq!(error(&value), "swarm_manifest_duplicate_identity");
        let mut value = manifest();
        value["nodes"][0]["tools"] = json!(["github.scope", "github.scope"]);
        assert_eq!(error(&value), "swarm_manifest_duplicate_tool");
    }

    #[test]
    fn refuses_unknown_fields_missing_fields_and_unsupported_versions() {
        let mut value = manifest();
        value["version"] = json!(2);
        assert_eq!(error(&value), "swarm_manifest_version_unsupported");
        let mut value = manifest();
        value["allow"] = json!(["*"]);
        assert_eq!(error(&value), "swarm_manifest_invalid");
        let mut value = manifest();
        value["nodes"][0]["allow"] = json!(["*"]);
        assert_eq!(error(&value), "swarm_manifest_invalid");
        let mut value = manifest();
        value["nodes"][0].as_object_mut().unwrap().remove("tools");
        assert_eq!(error(&value), "swarm_manifest_invalid");
        assert!(SwarmGraph::from_slice(br#"{"version":1,"version":1,"nodes":[]}"#).is_err());
    }

    #[test]
    fn rejects_invalid_identity_and_tool_shapes_without_echoing_them() {
        for id in ["", "with space", "line\nfeed", "nonascii-é"] {
            let mut value = manifest();
            value["nodes"][0]["id"] = json!(id);
            assert_eq!(error(&value), "swarm_manifest_agent_id_invalid");
        }
        for grant in [
            "",
            "0123456789ABCDEF",
            "../../etc/passwd",
            "lmt_0123456789abcdef.fake",
        ] {
            let mut value = manifest();
            value["nodes"][0]["grant_id"] = json!(grant);
            assert_eq!(error(&value), "swarm_manifest_grant_id_invalid");
        }
        for tool in [
            "",
            "*",
            "github.*",
            "github.scope\n",
            "github/scope",
            "nonascii-é",
        ] {
            let mut value = manifest();
            value["nodes"][0]["tools"] = json!([tool]);
            assert_eq!(error(&value), "swarm_manifest_tool_name_invalid");
        }
        let secretish = b"{\"version\": \"synthetic-sensitive-value\"}";
        let err = SwarmGraph::from_slice(secretish).unwrap_err().to_string();
        assert_eq!(err, "swarm_manifest_invalid");
        assert!(!err.contains("synthetic-sensitive-value"));
    }

    #[test]
    fn bounds_manifest_agent_and_tool_sizes() {
        assert_eq!(
            SwarmGraph::from_slice(&vec![b' '; MAX_MANIFEST_BYTES + 1])
                .unwrap_err()
                .to_string(),
            "swarm_manifest_too_large"
        );
        let mut value = manifest();
        value["nodes"] = json!(vec![value["nodes"][0].clone(); MAX_AGENTS + 1]);
        assert_eq!(error(&value), "swarm_manifest_too_many_agents");
        let mut value = manifest();
        value["nodes"][0]["tools"] = json!(vec!["github.scope"; MAX_TOOLS_PER_AGENT + 1]);
        assert_eq!(error(&value), "swarm_manifest_too_many_tools");
        let mut value = manifest();
        value["nodes"][0]["id"] = json!("x".repeat(MAX_AGENT_ID_BYTES + 1));
        assert_eq!(error(&value), "swarm_manifest_agent_id_invalid");
        let mut value = manifest();
        value["nodes"][0]["tools"] = json!(["x".repeat(MAX_TOOL_NAME_BYTES + 1)]);
        assert_eq!(error(&value), "swarm_manifest_tool_name_invalid");
    }

    #[test]
    fn load_is_bounded_and_retains_an_immutable_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.json");
        std::fs::write(&path, serde_json::to_vec(&manifest()).unwrap()).unwrap();
        let graph = SwarmGraph::load(&path).unwrap();
        std::fs::write(&path, b"{}").unwrap();
        assert!(graph.agent_for_grant("0123456789abcdef").is_some());
        assert!(SwarmGraph::load(&path).is_err());
        std::fs::write(&path, vec![b' '; MAX_MANIFEST_BYTES + 1]).unwrap();
        assert_eq!(
            SwarmGraph::load(&path).unwrap_err().to_string(),
            "swarm_manifest_too_large"
        );
        assert_eq!(
            SwarmGraph::load(dir.path()).unwrap_err().to_string(),
            "swarm_manifest_unreadable"
        );
        assert_eq!(
            SwarmGraph::load(&dir.path().join("missing"))
                .unwrap_err()
                .to_string(),
            "swarm_manifest_unreadable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_a_fifo_without_waiting_for_a_writer() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.fifo");
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: c_path is a live, NUL-terminated pathname in the test's
        // private temporary directory; no pointers are retained by mkfifo.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert_eq!(
            SwarmGraph::load(&path).unwrap_err().to_string(),
            "swarm_manifest_unreadable"
        );
    }
}
