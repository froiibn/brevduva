// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 리시버 평면의 영속 전달 기록 — 바인딩별 JSONL, 넘기기 전에 sync한다(P6). 형식은 옛 Desktop
//! 저널과 같아서 옛 어댑터가 남긴 기록도 같은 해독기로 읽는다(리시버 기동 때 잔재 점검, 7e).
use anyhow::Context as _;
use brevduva_protocol::{Envelope, Kind};
use serde::{Deserialize, Serialize};

/// 설정 디렉터리 아래의 기록 경로 — 옛 세션 어댑터(`desktop`·`claude-channel`·`native-session`·
/// `codex-cli`)가 쓰던 자리이고, 리시버가 기동할 때 그 잔재를 찾는 데 쓴다(7e).
pub(crate) fn journal_path(
    binding: &crate::config::Binding,
    adapter: &str,
) -> anyhow::Result<std::path::PathBuf> {
    journal_path_under(
        crate::config::config_path()?
            .parent()
            .context("config has no parent")?,
        binding,
        adapter,
    )
}

/// 뿌리 디렉터리를 직접 받는 판 (2026-09-10) — 리시버의 로컬 평면은 서비스 설정 디렉터리를,
/// 시험은 임시 디렉터리를 준다. 전역 설정 경로에 기대지 않아 시험이 사용자 설정을 건드리지 않는다.
pub(crate) fn journal_path_under(
    root: &std::path::Path,
    binding: &crate::config::Binding,
    adapter: &str,
) -> anyhow::Result<std::path::PathBuf> {
    for part in [
        adapter,
        binding.org.as_deref().unwrap_or("legacy"),
        &binding.agent,
        &binding.channel,
    ] {
        brevduva_protocol::Ident::parse(part)?;
    }
    Ok(root
        .join(adapter)
        .join(binding.org.as_deref().unwrap_or("legacy"))
        .join(&binding.agent)
        .join(&binding.channel)
        .join("deliveries.jsonl"))
}
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::Path;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
    pub(crate) server: String,
    pub(crate) binding: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryState {
    Pending,
    Submitting,
    Accepted,
    Unknown,
    Ignored,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Delivery {
    pub(crate) thread: String,
    pub(crate) envelope: Envelope,
    pub(crate) state: DeliveryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub(crate) enum JournalEntry {
    Identity { identity: Identity },
    Delivery { delivery: Box<Delivery> },
}

/// 파일 잠금은 종료 시 해제된다. 미완성 마지막 줄 외의 손상은 오류로 처리한다.
pub(crate) struct Journal {
    _lock: crate::file_lock::FileLock,
    file: File,
    pub(crate) entries: BTreeMap<String, Delivery>,
}

pub(crate) fn decode(
    bytes: &[u8],
    identity: &Identity,
) -> anyhow::Result<BTreeMap<String, Delivery>> {
    let mut entries = BTreeMap::new();
    let mut initialized = false;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        match serde_json::from_slice::<JournalEntry>(line).context("invalid delivery journal")? {
            JournalEntry::Identity { identity: saved } => {
                anyhow::ensure!(
                    !initialized && &saved == identity,
                    "delivery journal identity mismatch"
                );
                initialized = true;
            }
            JournalEntry::Delivery { delivery } => {
                anyhow::ensure!(initialized, "delivery journal has no identity header");
                entries.insert(envelope_id(&delivery.envelope)?.to_owned(), *delivery);
            }
        }
    }
    anyhow::ensure!(initialized, "delivery journal has no identity header");
    Ok(entries)
}

impl Journal {
    pub(crate) fn open(path: &Path, identity: Identity) -> anyhow::Result<Self> {
        // Windows 파일 잠금은 다른 프로세스의 읽기도 막으므로 데이터 파일과 분리한다.
        let lock = crate::file_lock::FileLock::acquire(&path.with_extension("lock"))
            .context("another receiver owns this delivery journal")?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // 넘기기 전에 sync하므로 미완성 마지막 줄은 잘라도 된다.
        // 확정 줄이 미완성이면 직전 submitting이 남아 자동 재주입을 막는다.
        let valid = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
        if valid != bytes.len() {
            file.set_len(valid as u64)?;
            file.sync_all()?;
            bytes.truncate(valid);
        }
        file.seek(std::io::SeekFrom::End(0))?;
        let entries = if bytes.is_empty() {
            BTreeMap::new()
        } else {
            decode(&bytes, &identity)?
        };
        let mut journal = Self {
            _lock: lock,
            file,
            entries,
        };
        if bytes.is_empty() {
            journal.append(&JournalEntry::Identity { identity })?;
        }
        Ok(journal)
    }

    fn append(&mut self, entry: &JournalEntry) -> anyhow::Result<()> {
        let mut bytes = serde_json::to_vec(entry)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file
            .sync_all()
            .context("delivery journal sync failed; nothing was confirmed")
    }

    pub(crate) fn store(&mut self, delivery: Delivery) -> anyhow::Result<()> {
        let id = envelope_id(&delivery.envelope)?.to_owned();
        self.append(&JournalEntry::Delivery {
            delivery: Box::new(delivery.clone()),
        })?;
        self.entries.insert(id, delivery);
        Ok(())
    }

    pub(crate) fn ingest(&mut self, thread: &str, envelope: Envelope) -> anyhow::Result<()> {
        if self.entries.contains_key(envelope_id(&envelope)?) {
            return Ok(());
        }
        // 프레즌스 등 시스템 이벤트는 모델을 깨우지 않되 수신 기록은 남긴다.
        let ignored = envelope.kind == Kind::Event || envelope.from.as_str().starts_with('_');
        self.store(Delivery {
            thread: thread.to_owned(),
            envelope,
            state: if ignored {
                DeliveryState::Ignored
            } else {
                DeliveryState::Pending
            },
            detail: None,
        })
    }
}

pub(crate) fn envelope_id(envelope: &Envelope) -> anyhow::Result<&str> {
    Ok(envelope
        .id
        .as_ref()
        .context("received envelope has no message ID")?
        .as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use brevduva_protocol::ClientKey;

    /// 기록 파일 자리 — 끝나면 지운다. (옛 Desktop 어댑터에서 옮긴 시험, 2026-09-11)
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("brv-journal-test-{}", ClientKey::generate()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join("journal.jsonl")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn identity() -> Identity {
        Identity {
            server: "https://test.invalid".into(),
            binding: "org/agent@channel".into(),
        }
    }

    fn envelope() -> Envelope {
        serde_json::from_value(
            serde_json::json!({"v":1,"id":ClientKey::generate(),"client_key":ClientKey::generate(),
            "from":"peer","to":"agent:agent","kind":"request","expects":"reply","hops":0,
            "content_type":"text/markdown","payload":"peer text","meta":{}}),
        )
        .unwrap()
    }

    #[test]
    fn a_recorded_message_is_not_recorded_twice_after_restart() {
        let fixture = Fixture::new();
        let message = envelope();
        {
            let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
            journal.ingest("session-a", message.clone()).unwrap();
        }
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        journal.ingest("session-b", message).unwrap();
        assert_eq!(journal.entries.len(), 1);
        assert_eq!(
            journal.entries.values().next().unwrap().thread,
            "session-a",
            "처음 넘겨받은 세션을 지킨다"
        );
    }

    #[test]
    fn a_torn_final_line_cannot_hide_an_uncertain_submission() {
        let fixture = Fixture::new();
        {
            let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
            journal.ingest("session-a", envelope()).unwrap();
            let mut delivery = journal.entries.values().next().unwrap().clone();
            delivery.state = DeliveryState::Submitting;
            journal.store(delivery).unwrap();
        }
        // 중간에 잘린 확정 기록은 넘겼는지 모르는 submitting을 덮지 못한다.
        OpenOptions::new()
            .append(true)
            .open(fixture.path())
            .unwrap()
            .write_all(b"{\"record\":")
            .unwrap();
        let journal = Journal::open(&fixture.path(), identity()).unwrap();
        assert_eq!(
            journal.entries.values().next().unwrap().state,
            DeliveryState::Submitting
        );
    }

    #[test]
    fn a_confirmed_message_is_never_added_as_pending_again() {
        let fixture = Fixture::new();
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        let message = envelope();
        journal.ingest("session-a", message.clone()).unwrap();
        let mut delivery = journal.entries.values().next().unwrap().clone();
        delivery.state = DeliveryState::Accepted;
        journal.store(delivery).unwrap();
        journal.ingest("session-b", message).unwrap();
        assert_eq!(
            journal.entries.values().next().unwrap().state,
            DeliveryState::Accepted
        );
    }

    #[test]
    fn a_journal_has_one_owner_and_one_identity() {
        let fixture = Fixture::new();
        let journal = Journal::open(&fixture.path(), identity()).unwrap();
        assert!(Journal::open(&fixture.path(), identity()).is_err());
        // 쓰는 중인 기록도 읽을 수는 있어야 한다 (Windows 잠금 회귀).
        assert!(decode(&std::fs::read(fixture.path()).unwrap(), &identity()).is_ok());
        drop(journal);
        let mut wrong = identity();
        wrong.server = "https://other.invalid".into();
        assert!(Journal::open(&fixture.path(), wrong).is_err());
    }

    #[test]
    fn missing_ids_and_corrupt_records_fail_closed() {
        let fixture = Fixture::new();
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        let mut message = envelope();
        message.id = None;
        assert!(journal.ingest("session", message).is_err());
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(fixture.path())
            .unwrap()
            .write_all(b"bad record\n")
            .unwrap();
        assert!(Journal::open(&fixture.path(), identity()).is_err());
    }
}
