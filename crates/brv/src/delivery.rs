// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 어댑터 공통 영속 전달 상태. 기존 Desktop 저널 형식은 유지한다.
use anyhow::Context as _;
use brevduva_protocol::{Envelope, Kind};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(crate) fn journal_path(
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
    Ok(crate::config::config_path()?
        .parent()
        .context("config has no parent")?
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
        match serde_json::from_slice::<JournalEntry>(line).context("invalid Desktop journal")? {
            JournalEntry::Identity { identity: saved } => {
                anyhow::ensure!(
                    !initialized && &saved == identity,
                    "Desktop journal identity mismatch"
                );
                initialized = true;
            }
            JournalEntry::Delivery { delivery } => {
                anyhow::ensure!(initialized, "Desktop journal has no identity header");
                entries.insert(envelope_id(&delivery.envelope)?.to_owned(), *delivery);
            }
        }
    }
    anyhow::ensure!(initialized, "Desktop journal has no identity header");
    Ok(entries)
}

impl Journal {
    pub(crate) fn open(path: &Path, identity: Identity) -> anyhow::Result<Self> {
        // Windows 파일 잠금은 다른 프로세스의 읽기도 막으므로 데이터 파일과 분리한다.
        let lock = crate::file_lock::FileLock::acquire(&path.with_extension("lock"))
            .context("another Desktop receiver owns this binding")?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // IPC 전송 전에 sync하므로 미완성 마지막 줄은 잘라도 된다.
        // accepted 줄이 미완성이면 직전 submitting이 남아 자동 재전송을 막는다.
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
            .context("Desktop journal sync failed; receipt not confirmed")
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

    pub(crate) fn validate_resume(&self, thread: &str) -> anyhow::Result<()> {
        for delivery in self.entries.values() {
            anyhow::ensure!(
                !matches!(
                    delivery.state,
                    DeliveryState::Submitting | DeliveryState::Unknown
                ),
                "delivery {} has an uncertain outcome; inspect Desktop history and desktop status before resolving the journal; automatic replay refused",
                envelope_id(&delivery.envelope)?
            );
            anyhow::ensure!(
                delivery.state != DeliveryState::Pending || delivery.thread == thread,
                "pending delivery {} belongs to another task; resume that exact --thread",
                envelope_id(&delivery.envelope)?
            );
        }
        Ok(())
    }

    pub(crate) fn resolve(
        &mut self,
        id: &str,
        turn: Option<&str>,
        note: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !note.trim().is_empty(),
            "record the evidence from inspecting the exact task history"
        );
        anyhow::ensure!(
            turn.is_none_or(|s| !s.trim().is_empty()),
            "turn ID must not be empty"
        );
        let mut delivery = self
            .entries
            .get(id)
            .context("delivery ID not found")?
            .clone();
        anyhow::ensure!(
            matches!(
                delivery.state,
                DeliveryState::Submitting | DeliveryState::Unknown
            ),
            "only uncertain deliveries can be resolved"
        );
        delivery.state = if turn.is_some() {
            DeliveryState::Accepted
        } else {
            DeliveryState::Pending
        };
        delivery.detail = Some(
            json!({"resolution":"operator_verified", "turn_id":turn, "note":note}).to_string(),
        );
        self.store(delivery)
    }
}

pub(crate) fn envelope_id(envelope: &Envelope) -> anyhow::Result<&str> {
    Ok(envelope
        .id
        .as_ref()
        .context("received envelope has no message ID")?
        .as_str())
}
