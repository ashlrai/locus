//! Bounded, in-memory peer relay. Transport/session authentication belongs to
//! the caller; private session keys and tenant partitions never leave this API.
//! Messages attest the registered session, not the truth of client metadata.

use base64::{engine::general_purpose, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;

pub const MAX_PEERS: usize = 256;
pub const MAX_THREADS: usize = 256;
pub const MAX_MESSAGES: usize = 1024;
pub const MAX_BODY_BYTES: usize = 8192;
pub const MAX_MEMBERS: usize = 16;
pub const MAX_PAGE_SIZE: usize = 100;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PeerError {
    #[error("peer session is not registered")]
    Unregistered,
    #[error("peer or thread is unavailable")]
    Unavailable,
    #[error("unknown engine is read-only")]
    ReadOnly,
    #[error("peer registration is immutable for this session")]
    RegistrationChanged,
    #[error("peer relay capacity reached")]
    Capacity,
    #[error("invalid peer relay arguments")]
    Invalid,
    #[error("text contains a secret-shaped value; delivery refused")]
    SecretShape,
    #[error("registered process is not alive")]
    DeadProcess,
}

type Result<T> = std::result::Result<T, PeerError>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub engine_id: String,
    pub task: String,
    pub cwd: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub engine_id: String,
    pub task: String,
    pub cwd: String,
    pub pid: u32,
    /// Assigned by the authenticated transport, never by message arguments.
    pub trusted: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Message {
    pub id: u64,
    pub sender: Peer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PeerThread {
    pub id: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThreadView {
    pub thread: PeerThread,
    pub messages: Vec<Message>,
}

struct Entry {
    partition: String,
    peer: Peer,
}

/// No disk persistence: restart clears both registration and message history.
/// At capacity, the oldest message is evicted; message ids never repeat.
#[derive(Default)]
pub struct PeerRegistry {
    peers: HashMap<String, Entry>,
    threads: HashMap<String, PeerThread>,
    messages: VecDeque<Message>,
    last_message_id: u64,
}

impl PeerRegistry {
    pub fn register(
        &mut self,
        session_key: &str,
        partition: &str,
        registration: Registration,
        trusted: bool,
    ) -> Result<Peer> {
        if session_key.is_empty() || partition.is_empty() {
            return Err(PeerError::Invalid);
        }
        validate_text(&registration.engine_id, 128, false)?;
        validate_text(&registration.task, 512, false)?;
        validate_text(&registration.cwd, 4096, false)?;
        if !process_alive(registration.pid) {
            return Err(PeerError::DeadProcess);
        }
        if let Some(entry) = self.peers.get(session_key) {
            let p = &entry.peer;
            if entry.partition != partition
                || p.engine_id != registration.engine_id
                || p.task != registration.task
                || p.cwd != registration.cwd
                || p.pid != registration.pid
                || p.trusted != trusted
            {
                return Err(PeerError::RegistrationChanged);
            }
            return Ok(p.clone());
        }
        if self.peers.len() >= MAX_PEERS {
            return Err(PeerError::Capacity);
        }
        let peer = Peer {
            id: random_id("peer"),
            engine_id: registration.engine_id,
            task: registration.task,
            cwd: registration.cwd,
            pid: registration.pid,
            trusted,
        };
        self.peers.insert(
            session_key.to_owned(),
            Entry {
                partition: partition.to_owned(),
                peer: peer.clone(),
            },
        );
        Ok(peer)
    }

    pub fn own(&self, session_key: &str) -> Result<Peer> {
        Ok(self.entry(session_key)?.peer.clone())
    }

    pub fn list(&self, session_key: &str) -> Result<Vec<Peer>> {
        let caller = self.entry(session_key)?;
        let mut peers: Vec<_> = self
            .peers
            .values()
            .filter(|entry| entry.partition == caller.partition)
            .map(|entry| entry.peer.clone())
            .collect();
        peers.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(peers)
    }

    pub fn send(
        &mut self,
        session_key: &str,
        recipient: Option<&str>,
        thread: Option<&str>,
        body: &str,
    ) -> Result<Message> {
        let caller = self.writer(session_key)?;
        validate_text(body, MAX_BODY_BYTES, true)?;
        match (recipient, thread) {
            (Some(id), None) => {
                self.peer_in_partition(id, &caller.partition)?;
            }
            (None, Some(id)) => {
                self.member_thread(id, &caller.peer.id)?;
            }
            _ => return Err(PeerError::Invalid),
        }
        let message = Message {
            id: self
                .last_message_id
                .checked_add(1)
                .ok_or(PeerError::Capacity)?,
            sender: caller.peer.clone(),
            recipient: recipient.map(str::to_owned),
            thread: thread.map(str::to_owned),
            body: body.to_owned(),
        };
        self.last_message_id = message.id;
        if self.messages.len() == MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.messages.push_back(message.clone());
        Ok(message)
    }

    /// Returns direct deliveries and group messages for the caller only.
    pub fn inbox(&self, session_key: &str, after: u64, limit: usize) -> Result<Vec<Message>> {
        let caller = self.entry(session_key)?;
        validate_limit(limit)?;
        Ok(self
            .messages
            .iter()
            .filter(|message| {
                message.id > after
                    && (message.recipient.as_deref() == Some(&caller.peer.id)
                        || message
                            .thread
                            .as_ref()
                            .is_some_and(|id| self.member_thread(id, &caller.peer.id).is_ok()))
            })
            .take(limit)
            .cloned()
            .collect())
    }

    /// The creator is included automatically. Membership is immutable: a new
    /// session can never inherit an old peer id or its private thread history.
    pub fn create_thread(&mut self, session_key: &str, members: Vec<String>) -> Result<PeerThread> {
        let caller = self.writer(session_key)?;
        if members.is_empty() || members.len() > MAX_MEMBERS {
            return Err(PeerError::Invalid);
        }
        let mut unique: HashSet<_> = members.into_iter().collect();
        unique.insert(caller.peer.id.clone());
        if unique.len() < 2 || unique.len() > MAX_MEMBERS {
            return Err(PeerError::Invalid);
        }
        for id in &unique {
            self.peer_in_partition(id, &caller.partition)?;
        }
        if self.threads.len() >= MAX_THREADS {
            return Err(PeerError::Capacity);
        }
        let mut members: Vec<_> = unique.into_iter().collect();
        members.sort();
        let thread = PeerThread {
            id: random_id("thread"),
            members,
        };
        self.threads.insert(thread.id.clone(), thread.clone());
        Ok(thread)
    }

    pub fn thread(
        &self,
        session_key: &str,
        thread: &str,
        after: u64,
        limit: usize,
    ) -> Result<ThreadView> {
        let caller = self.entry(session_key)?;
        validate_limit(limit)?;
        let thread = self.member_thread(thread, &caller.peer.id)?;
        Ok(ThreadView {
            thread: thread.clone(),
            messages: self
                .messages
                .iter()
                .filter(|message| message.id > after && message.thread.as_ref() == Some(&thread.id))
                .take(limit)
                .cloned()
                .collect(),
        })
    }

    /// The transport checks its existing HTTP map here; probing must not
    /// refresh session TTL. PID liveness is an additional necessary condition.
    pub fn prune(&mut self, mut session_live: impl FnMut(&str) -> bool) -> usize {
        let before = self.peers.len();
        self.peers
            .retain(|key, entry| session_live(key) && process_alive(entry.peer.pid));
        self.prune_history();
        before - self.peers.len()
    }

    pub fn remove(&mut self, session_key: &str) {
        self.peers.remove(session_key);
        self.prune_history();
    }

    fn prune_history(&mut self) {
        let alive: HashSet<_> = self.peers.values().map(|e| e.peer.id.as_str()).collect();
        // Retire an entire group if any member dies, so history and membership
        // never silently change underneath its surviving participants.
        self.threads
            .retain(|_, thread| thread.members.iter().all(|id| alive.contains(id.as_str())));
        self.messages.retain(|message| {
            alive.contains(message.sender.id.as_str())
                && message
                    .recipient
                    .as_ref()
                    .is_none_or(|id| alive.contains(id.as_str()))
                && message
                    .thread
                    .as_ref()
                    .is_none_or(|id| self.threads.contains_key(id))
        });
    }

    fn entry(&self, session_key: &str) -> Result<&Entry> {
        self.peers.get(session_key).ok_or(PeerError::Unregistered)
    }

    fn writer(&self, session_key: &str) -> Result<&Entry> {
        let entry = self.entry(session_key)?;
        if !entry.peer.trusted {
            return Err(PeerError::ReadOnly);
        }
        Ok(entry)
    }

    fn peer_in_partition(&self, id: &str, partition: &str) -> Result<&Peer> {
        self.peers
            .values()
            .find(|entry| entry.peer.id == id && entry.partition == partition)
            .map(|entry| &entry.peer)
            .ok_or(PeerError::Unavailable)
    }

    fn member_thread(&self, id: &str, peer_id: &str) -> Result<&PeerThread> {
        self.threads
            .get(id)
            .filter(|thread| thread.members.iter().any(|member| member == peer_id))
            .ok_or(PeerError::Unavailable)
    }
}

fn random_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}

fn validate_limit(limit: usize) -> Result<()> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        Err(PeerError::Invalid)
    } else {
        Ok(())
    }
}

/// Shape screening is deliberately conservative and is not a DLP guarantee.
/// Refuse the whole message, never deliver partially redacted content.
pub fn validate_text(text: &str, cap: usize, multiline: bool) -> Result<()> {
    if text.trim().is_empty()
        || text.len() > cap
        || text
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
    {
        return Err(PeerError::Invalid);
    }
    let lower = text.to_ascii_lowercase();
    if [
        "sk-",
        "sk_live_",
        "sk_test_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "xoxa-",
        "xoxr-",
        "phm_",
        "sb_secret_",
        "lmt_",
    ]
    .iter()
    .any(|shape| {
        lower.match_indices(shape).any(|(index, _)| {
            index == 0
                || !(lower.as_bytes()[index - 1].is_ascii_alphanumeric()
                    || lower.as_bytes()[index - 1] == b'_')
        })
    }) {
        return Err(PeerError::SecretShape);
    }
    let words: Vec<_> = lower.split_whitespace().collect();
    if ["bearer", "basic"].iter().any(|scheme| {
        lower.match_indices(scheme).any(|(index, _)| {
            (index == 0 || !lower.as_bytes()[index - 1].is_ascii_alphanumeric())
                && lower[index + scheme.len()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_whitespace)
                && (*scheme == "bearer" || basic_credential_shape(&text[index + scheme.len()..]))
        })
    }) || (lower.contains("-----begin")
        && words
            .windows(2)
            .any(|pair| pair[0] == "private" && pair[1].starts_with("key")))
    {
        return Err(PeerError::SecretShape);
    }
    // Tokenize independently of JSON punctuation; quoted/embedded JWTs must
    // receive the same screening as a bare token.
    if text
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .any(|token| {
            let segments: Vec<_> = token.trim_matches('.').split('.').collect();
            segments.len() == 3 && segments.iter().all(|part| part.len() >= 8)
        })
    {
        return Err(PeerError::SecretShape);
    }
    // Decode JSON before calling this function. Scan assignments independently
    // of quoting and whitespace, including shell-style exports and JSON keys.
    for (index, character) in lower.char_indices() {
        if !matches!(character, ':' | '=') {
            continue;
        }
        let key = lower[..index]
            .trim_end_matches(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .rsplit(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
            .next()
            .unwrap_or("");
        if sensitive_key(key) {
            return Err(PeerError::SecretShape);
        }
    }
    for word in text.split(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '{' | '}' | '(' | ')')
    }) {
        let word = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | ',' | ';'));
        if word.as_bytes().windows(20).any(|candidate| {
            (candidate.starts_with(b"AKIA") || candidate.starts_with(b"ASIA"))
                && candidate.iter().all(|c| c.is_ascii_alphanumeric())
        }) {
            return Err(PeerError::SecretShape);
        }
        let url_text = if let Some(scheme_end) = word.find("://") {
            let scheme_start = word[..scheme_end]
                .char_indices()
                .rev()
                .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
                .map_or(0, |(index, character)| index + character.len_utf8());
            &word[scheme_start..]
        } else {
            word
        };
        if let Ok(url) = url::Url::parse(url_text) {
            if !url.username().is_empty()
                || url.password().is_some()
                || url
                    .query_pairs()
                    .any(|(key, _)| sensitive_key(&key.to_ascii_lowercase()))
            {
                return Err(PeerError::SecretShape);
            }
        }
    }
    Ok(())
}

fn sensitive_key(key: &str) -> bool {
    matches!(
        key,
        "key"
            | "token"
            | "secret"
            | "password"
            | "passwd"
            | "authorization"
            | "credential"
            | "credentials"
            | "api-key"
            | "apikey"
            | "bearer"
            | "signature"
            | "accesstoken"
            | "refreshtoken"
            | "clientsecret"
            | "privatekey"
            | "auth"
            | "sig"
            | "cookie"
    ) || [
        "_key",
        "_token",
        "_secret",
        "_password",
        "_credential",
        "-key",
        "-token",
        "-secret",
        "-password",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix))
}

fn basic_credential_shape(after_scheme: &str) -> bool {
    let token = after_scheme
        .trim_start()
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '}' | ')' | ',' | ';'))
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    general_purpose::STANDARD
        .decode(token)
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(token))
        .is_ok_and(|decoded| zeroize::Zeroizing::new(decoded).contains(&b':'))
}

/// Only positive single-process PIDs are accepted (never kill process groups).
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    #[cfg(unix)]
    {
        // SAFETY: signal zero checks liveness without delivering a signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // Unsupported process inspection cannot establish liveness.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> Registration {
        Registration {
            engine_id: "codex".into(),
            task: "synthetic test task".into(),
            cwd: "/tmp/peer-fixture".into(),
            pid: std::process::id(),
        }
    }

    fn register(registry: &mut PeerRegistry, key: &str, partition: &str, trusted: bool) -> Peer {
        registry
            .register(key, partition, metadata(), trusted)
            .unwrap()
    }

    #[test]
    fn delivery_attests_sender_and_hides_session_keys() {
        let mut registry = PeerRegistry::default();
        let alice = register(&mut registry, "private-session-alice", "tenant-a", true);
        let bob = register(&mut registry, "private-session-bob", "tenant-a", true);
        let message = registry
            .send(
                "private-session-alice",
                Some(&bob.id),
                None,
                "Ready for review",
            )
            .unwrap();
        assert_eq!(message.sender, alice);
        assert_eq!(
            registry.inbox("private-session-bob", 0, 10).unwrap(),
            vec![message.clone()]
        );
        assert!(registry
            .inbox("private-session-alice", 0, 10)
            .unwrap()
            .is_empty());
        let encoded = serde_json::to_string(&message).unwrap();
        assert!(!encoded.contains("private-session"));
        assert!(!encoded.contains("tenant-a"));
    }

    #[test]
    fn registration_is_immutable_and_idempotent() {
        let mut registry = PeerRegistry::default();
        let first = register(&mut registry, "session", "tenant", false);
        assert_eq!(register(&mut registry, "session", "tenant", false), first);
        assert_eq!(
            registry.register("session", "tenant", metadata(), true),
            Err(PeerError::RegistrationChanged)
        );
        assert_eq!(
            registry.register("session", "other-tenant", metadata(), false),
            Err(PeerError::RegistrationChanged)
        );
        let mut changed = metadata();
        changed.engine_id = "claude".into();
        assert_eq!(
            registry.register("session", "tenant", changed, false),
            Err(PeerError::RegistrationChanged)
        );
    }

    #[test]
    fn unknown_engine_cannot_send_to_thread_or_create_one() {
        let mut registry = PeerRegistry::default();
        let alice = register(&mut registry, "alice", "tenant", true);
        let unknown = register(&mut registry, "unknown", "tenant", false);
        let thread = registry
            .create_thread("alice", vec![unknown.id.clone()])
            .unwrap();
        registry
            .send("alice", None, Some(&thread.id), "Review this change")
            .unwrap();
        assert_eq!(
            registry.send("unknown", None, Some(&thread.id), "reply"),
            Err(PeerError::ReadOnly)
        );
        assert_eq!(
            registry.send("unknown", Some(&alice.id), None, "reply"),
            Err(PeerError::ReadOnly)
        );
        assert_eq!(
            registry.create_thread("unknown", vec![alice.id]),
            Err(PeerError::ReadOnly)
        );
        assert_eq!(
            registry
                .thread("unknown", &thread.id, 0, 10)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert_eq!(registry.list("unknown").unwrap().len(), 2);
    }

    #[test]
    fn partition_and_thread_membership_are_enforced() {
        let mut registry = PeerRegistry::default();
        register(&mut registry, "alice", "tenant-a", true);
        let bob = register(&mut registry, "bob", "tenant-a", true);
        register(&mut registry, "outsider", "tenant-a", true);
        let foreign = register(&mut registry, "foreign", "tenant-b", true);
        let thread = registry.create_thread("alice", vec![bob.id]).unwrap();
        assert_eq!(registry.list("foreign").unwrap(), vec![foreign.clone()]);
        assert_eq!(
            registry.send("alice", Some(&foreign.id), None, "hello"),
            Err(PeerError::Unavailable)
        );
        assert_eq!(
            registry.create_thread("alice", vec![foreign.id]),
            Err(PeerError::Unavailable)
        );
        for key in ["outsider", "foreign"] {
            assert_eq!(
                registry.send(key, None, Some(&thread.id), "hello"),
                Err(PeerError::Unavailable)
            );
            assert!(matches!(
                registry.thread(key, &thread.id, 0, 10),
                Err(PeerError::Unavailable)
            ));
        }
    }

    #[test]
    fn secret_shaped_body_refused_without_delivery_or_echo() {
        let mut registry = PeerRegistry::default();
        register(&mut registry, "alice", "tenant", true);
        let bob = register(&mut registry, "bob", "tenant", true);
        // Build sensitive fixtures at runtime: source scans also cover cfg(test).
        // Keep the assembled bytes unchanged so refusal and non-echo coverage is preserved.
        let shaped = [
            ["sk", "-", "synthetic", "-", "fixture"].concat(),
            ["ghp", "_", "synthetic", "_", "fixture"].concat(),
            ["github", "_", "pat", "_", "fixture"].concat(),
            ["Authorization", ": ", "Bearer", " ", "synthetic"].concat(),
            ["password", " = ", "'synthetic'"].concat(),
            ["{\"api", "_", "key\"", ":", "\"synthetic\"}"].concat(),
            ["ACCESS", "_", "TOKEN", "=", "synthetic"].concat(),
            ["{\"apiKey\"", ":", "\"synthetic\"}"].concat(),
            ["{\"accessToken\"", ":", "\"synthetic\"}"].concat(),
            ["Bearer", "\t", "synthetic"].concat(),
            ["Basic", " ", "dXNlcjpwYXNz"].concat(),
            ["{\"value\"", ":", "\"Basic", "\t", "dXNlcjpwYXNz\"}"].concat(),
            [
                "-----", "BEGIN", " ", "RSA", " ", "PRIVATE", "\n", "KEY", "-----",
            ]
            .concat(),
            ["lmt", "_", "synthetic", "_", "fixture"].concat(),
            ["{\"value\"", ":", "\"Bearer", "\t", "synthetic\"}"].concat(),
            ["-----", "BEGIN", " ", "PRIVATE", " ", "KEY", "-----"].concat(),
            ["AKIA", "0000000000000000"].concat(),
            [
                "https",
                "://",
                "user",
                ":",
                "synthetic",
                "@",
                "example",
                ".",
                "test",
            ]
            .concat(),
            [
                "url",
                "=",
                "https",
                "://",
                "user",
                ":",
                "synthetic",
                "@",
                "example",
                ".",
                "test",
            ]
            .concat(),
            [
                "☃https",
                "://",
                "user",
                ":",
                "synthetic",
                "@",
                "example",
                ".",
                "test",
            ]
            .concat(),
            [
                "--",
                "proxy",
                "=",
                "https",
                "://",
                "user",
                ":",
                "synthetic",
                "@",
                "example",
                ".",
                "test",
            ]
            .concat(),
            ["first", "=", "ok;CLIENT", "-", "SECRET", "=", "'synthetic'"].concat(),
            [
                "https",
                "://",
                "example",
                ".",
                "test",
                "/?",
                "auth",
                "=",
                "synthetic",
            ]
            .concat(),
            [
                "url",
                "=",
                "https",
                "://",
                "example",
                ".",
                "test",
                "/?",
                "q",
                "=",
                "normal&%74oken",
                "=",
                "synthetic",
            ]
            .concat(),
            [
                "{\"url\"",
                ":",
                "\"https",
                "://",
                "user",
                ":",
                "synthetic",
                "@",
                "example",
                ".",
                "test\"}",
            ]
            .concat(),
            [
                "https",
                "://",
                "example",
                ".",
                "test",
                "/?",
                "token",
                "=",
                "synthetic",
            ]
            .concat(),
            [
                "eyJhbGciOiJIUzI1NiJ9",
                ".",
                "eyJzdWIiOiJmaXh0dXJlIn0",
                ".",
                "c3ludGhldGlj",
            ]
            .concat(),
            [
                "Use",
                " ",
                "eyJhbGciOiJIUzI1NiJ9",
                ".",
                "eyJzdWIiOiJmaXh0dXJlIn0",
                ".",
                "c3ludGhldGlj",
                ".",
            ]
            .concat(),
            [
                "{\"data\"",
                ":",
                "\"eyJhbGciOiJIUzI1NiJ9",
                ".",
                "eyJzdWIiOiJmaXh0dXJlIn0",
                ".",
                "c3ludGhldGlj\"}",
            ]
            .concat(),
        ];
        for body in shaped {
            let error = registry
                .send("alice", Some(&bob.id), None, &body)
                .unwrap_err();
            assert_eq!(error, PeerError::SecretShape);
            assert!(!error.to_string().contains(&body));
        }
        assert!(registry.inbox("bob", 0, 10).unwrap().is_empty());
    }

    #[test]
    fn metadata_uses_same_secret_screen() {
        for field in 0..3 {
            let mut registration = metadata();
            match field {
                0 => registration.engine_id = ["ghp", "_", "synthetic"].concat(),
                1 => registration.task = ["api", "_", "key", "=", "synthetic"].concat(),
                _ => registration.cwd = ["/", "tmp", "/", "ghp", "_", "synthetic"].concat(),
            }
            assert_eq!(
                PeerRegistry::default().register("session", "tenant", registration, true),
                Err(PeerError::SecretShape)
            );
        }
    }

    #[test]
    fn ordinary_task_paths_and_prose_are_accepted() {
        assert!(validate_text("Basic checks passed", MAX_BODY_BYTES, true).is_ok());
        let mut registry = PeerRegistry::default();
        let mut registration = metadata();
        registration.task = "task-123: review Asia rollout".into();
        registration.cwd = "/tmp/task-123".into();
        assert!(registry
            .register("session", "tenant", registration, true)
            .is_ok());
        assert!(validate_text(
            "Review task-123 for risk-based routing",
            MAX_BODY_BYTES,
            true
        )
        .is_ok());
    }

    #[test]
    fn text_only_byte_cap_and_controls_enforced() {
        let mut registry = PeerRegistry::default();
        register(&mut registry, "alice", "tenant", true);
        let bob = register(&mut registry, "bob", "tenant", true);
        for body in [
            "".to_owned(),
            " ".to_owned(),
            "hello\0world".to_owned(),
            "hello\u{1b}[31m".to_owned(),
            "é".repeat(MAX_BODY_BYTES / 2 + 1),
        ] {
            assert_eq!(
                registry.send("alice", Some(&bob.id), None, &body),
                Err(PeerError::Invalid)
            );
        }
        assert!(registry
            .send("alice", Some(&bob.id), None, &"x".repeat(MAX_BODY_BYTES))
            .is_ok());
        assert!(registry
            .send("alice", Some(&bob.id), None, "first\nsecond\tcolumn")
            .is_ok());
        assert!(validate_text("first\nsecond", 100, false).is_err());
    }

    #[test]
    fn malformed_destinations_and_pages_fail_closed() {
        let mut registry = PeerRegistry::default();
        let alice = register(&mut registry, "alice", "tenant", true);
        assert_eq!(
            registry.send("alice", None, None, "hello"),
            Err(PeerError::Invalid)
        );
        assert_eq!(
            registry.send("alice", Some(&alice.id), Some("thread"), "hello"),
            Err(PeerError::Invalid)
        );
        assert_eq!(
            registry.send("missing", Some(&alice.id), None, "hello"),
            Err(PeerError::Unregistered)
        );
        assert_eq!(registry.inbox("alice", 0, 0), Err(PeerError::Invalid));
        assert_eq!(registry.inbox("alice", 0, 101), Err(PeerError::Invalid));
        assert_eq!(
            registry.create_thread("alice", vec![]),
            Err(PeerError::Invalid)
        );
        assert_eq!(
            registry.create_thread("alice", vec![alice.id]),
            Err(PeerError::Invalid)
        );
    }

    #[test]
    fn cursor_pages_and_history_capacity_are_bounded() {
        let mut registry = PeerRegistry::default();
        register(&mut registry, "alice", "tenant", true);
        let bob = register(&mut registry, "bob", "tenant", true);
        for _ in 0..MAX_MESSAGES + 1 {
            registry
                .send("alice", Some(&bob.id), None, "hello")
                .unwrap();
        }
        assert_eq!(registry.messages.len(), MAX_MESSAGES);
        let first = registry.inbox("bob", 0, 2).unwrap();
        assert_eq!(first.iter().map(|m| m.id).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(registry.inbox("bob", 3, 1).unwrap()[0].id, 4);
        assert!(registry.inbox("bob", u64::MAX, 10).unwrap().is_empty());
    }

    #[test]
    fn session_expiry_removes_history_and_reinitialization_has_new_identity() {
        let mut registry = PeerRegistry::default();
        register(&mut registry, "alice", "tenant", true);
        let bob = register(&mut registry, "bob", "tenant", true);
        let thread = registry
            .create_thread("alice", vec![bob.id.clone()])
            .unwrap();
        registry
            .send("alice", None, Some(&thread.id), "hello")
            .unwrap();
        registry
            .send("alice", Some(&bob.id), None, "direct")
            .unwrap();
        assert_eq!(registry.prune(|key| key != "bob"), 1);
        assert!(registry.messages.is_empty());
        assert!(registry.threads.is_empty());
        let new = register(&mut registry, "bob", "tenant", true);
        assert_ne!(new.id, bob.id);
        assert!(registry.inbox("bob", 0, 100).unwrap().is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn dead_pid_is_pruned() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let mut registration = metadata();
        registration.pid = child.id();
        let mut registry = PeerRegistry::default();
        registry
            .register("child", "tenant", registration.clone(), true)
            .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(registry.prune(|_| true), 1);
        assert_eq!(registry.own("child"), Err(PeerError::Unregistered));
        assert_eq!(
            registry.register("dead", "tenant", registration, true),
            Err(PeerError::DeadProcess)
        );
        assert!(!process_alive(0));
        assert!(!process_alive(u32::MAX));
    }

    #[test]
    fn peer_thread_and_membership_capacities_are_enforced() {
        let mut registry = PeerRegistry::default();
        for i in 0..MAX_PEERS {
            register(&mut registry, &i.to_string(), "tenant", true);
        }
        assert_eq!(
            registry.register("overflow", "tenant", metadata(), true),
            Err(PeerError::Capacity)
        );
        let peer = registry.own("1").unwrap();
        for _ in 0..MAX_THREADS {
            registry.create_thread("0", vec![peer.id.clone()]).unwrap();
        }
        assert_eq!(
            registry.create_thread("0", vec![peer.id]),
            Err(PeerError::Capacity)
        );
        let members = registry
            .list("0")
            .unwrap()
            .into_iter()
            .take(MAX_MEMBERS + 1)
            .map(|p| p.id)
            .collect();
        assert_eq!(
            registry.create_thread("0", members),
            Err(PeerError::Invalid)
        );
    }

    #[test]
    fn removal_retires_threads_and_direct_messages() {
        let mut registry = PeerRegistry::default();
        register(&mut registry, "alice", "tenant", true);
        let bob = register(&mut registry, "bob", "tenant", true);
        let thread = registry
            .create_thread("alice", vec![bob.id.clone()])
            .unwrap();
        registry
            .send("alice", None, Some(&thread.id), "group")
            .unwrap();
        registry
            .send("alice", Some(&bob.id), None, "direct")
            .unwrap();
        registry.remove("alice");
        assert!(registry.inbox("bob", 0, 10).unwrap().is_empty());
        assert!(matches!(
            registry.thread("bob", &thread.id, 0, 10),
            Err(PeerError::Unavailable)
        ));
    }
}
