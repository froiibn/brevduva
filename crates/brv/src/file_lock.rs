// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// Unix에서 복제된 파일 핸들이 남아 있어도 소유 범위 종료 시 잠금을 해제한다.
/// 프로세스 생성 중 fork가 핸들을 일시 상속하므로 close에만 의존하면 해제가 늦어질 수 있다.
pub(crate) struct FileLock(File);

impl FileLock {
    pub(crate) fn acquire(path: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()?;
        Ok(Self(file))
    }

    pub(crate) fn held(path: &Path) -> anyhow::Result<bool> {
        match std::fs::metadata(path) {
            Ok(metadata) => anyhow::ensure!(metadata.is_file(), "lock path is not a regular file"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
        let file = match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        match file.try_lock() {
            Ok(()) => {
                drop(Self(file));
                Ok(false)
            }
            Err(TryLockError::WouldBlock) => Ok(true),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::error!(%error, "file lock release failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn release_does_not_wait_for_a_duplicated_descriptor_to_close() {
        let path = std::env::temp_dir().join(format!(
            "brv-lock-{}",
            brevduva_protocol::ClientKey::generate()
        ));
        // fork와 같은 open-file-description 공유를 재현한다. File의 close만으로는 해제되지 않는다.
        let raw = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        raw.try_lock().unwrap();
        let inherited = raw.try_clone().unwrap();
        drop(raw);
        assert!(FileLock::held(&path).unwrap());
        inherited.unlock().unwrap();
        drop(inherited);
        let guard = FileLock::acquire(&path).unwrap();
        let inherited = guard.0.try_clone().unwrap();
        assert!(FileLock::held(&path).unwrap());
        drop(guard);
        assert!(!FileLock::held(&path).unwrap());
        drop(inherited);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_lock_path_is_an_error_not_a_live_worker() {
        let dir = std::env::temp_dir();
        assert!(FileLock::held(&dir).is_err());
    }
}
