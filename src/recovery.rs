use crate::protocol;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub product_id: u16,
    pub serial: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    device: Identity,
    slot: u8,
    baseline: Vec<u8>,
}

pub trait Keyboard {
    fn active_slot(&self) -> Result<u8>;
    fn stored_profile(&self, slot: u8) -> Result<Vec<u8>>;
    fn write_profile(&self, slot: u8, body: &[u8]) -> Result<()>;
    fn colors(&self) -> Result<Vec<[u8; 3]>>;
}

/// One lock and recovery location shared by all invocations, regardless of cwd.
pub struct Recovery {
    path: PathBuf,
    _lock: File,
}

impl Recovery {
    pub fn acquire(directory: &Path) -> Result<Self> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory)?;
        ensure!(
            fs::symlink_metadata(directory)?.file_type().is_dir(),
            "recovery directory must not be a symlink"
        );
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(directory.join("session.lock"))?;
        lock.try_lock()
            .context("another Glance command is running")?;
        Ok(Self {
            path: directory.join("recovery.json"),
            _lock: lock,
        })
    }
}

pub fn preview<K: Keyboard>(
    keyboard: &K,
    identity: &Identity,
    recovery: &Recovery,
    leds: &[(u8, [u8; 3])],
    during: impl FnOnce() -> Result<()>,
) -> Result<Vec<[u8; 3]>> {
    with_overlay(keyboard, identity, recovery, leds, |_, _| during())
}

pub fn with_overlay<K: Keyboard>(
    keyboard: &K,
    identity: &Identity,
    recovery: &Recovery,
    leds: &[(u8, [u8; 3])],
    during: impl FnOnce(u8, &[u8]) -> Result<()>,
) -> Result<Vec<[u8; 3]>> {
    let path = &recovery.path;
    ensure!(
        !identity.serial.is_empty(),
        "a device serial number is required for recovery"
    );
    let slot = keyboard.active_slot()?;
    ensure!(slot < 4, "only onboard profiles are supported");
    let baseline = keyboard.stored_profile(slot)?;
    let overlay = protocol::preview(&baseline, leds)?;
    let record = Record {
        version: 1,
        device: identity.clone(),
        slot,
        baseline,
    };
    save(path, &record)?;
    // From here, every ordinary error passes through restoration, including a
    // partially successful upload. A force kill leaves the durable record.
    let operation = (|| {
        ensure!(
            keyboard.active_slot()? == slot,
            "active profile changed before preview"
        );
        ensure!(
            keyboard.stored_profile(slot)? == record.baseline,
            "saved settings changed before preview"
        );
        keyboard.write_profile(slot, &overlay)?;
        during(slot, &record.baseline)
    })();
    let restoration = restore(keyboard, identity, recovery);
    match (operation, restoration) {
        (Ok(()), Ok(colors)) => Ok(colors),
        (Err(error), Ok(_)) => {
            Err(error.context("preview failed; saved lighting was uploaded and read back"))
        }
        (operation, Err(error)) => {
            let preceding = operation
                .err()
                .map(|e| format!(" Preview error: {e:#}."))
                .unwrap_or_default();
            Err(error.context(format!("restoration failed; recovery record retained at {}.{preceding} Run `glance restore` after reconnecting", path.display())))
        }
    }
}

pub fn restore<K: Keyboard>(
    keyboard: &K,
    identity: &Identity,
    recovery: &Recovery,
) -> Result<Vec<[u8; 3]>> {
    let path = &recovery.path;
    let record = load(path)?;
    ensure!(
        record.version == 1 && record.slot < 4,
        "unsupported recovery record"
    );
    ensure!(
        &record.device == identity && !identity.serial.is_empty(),
        "recovery record belongs to a different keyboard"
    );
    protocol::baseline(&record.baseline)?;
    // Saved Wootility settings may have changed since the preview or a crash.
    // Re-read them, never roll back another application's saved edits.
    let current = keyboard.stored_profile(record.slot)?;
    protocol::baseline(&current)?;
    keyboard.write_profile(record.slot, &current)?;
    ensure!(
        keyboard.stored_profile(record.slot)? == current,
        "saved profile changed during restoration; retry restore"
    );
    let colors = keyboard.colors()?;
    ensure!(
        colors.len() == protocol::LED_COUNT,
        "restoration readback must contain ten colors"
    );
    fs::remove_file(path)
        .context("lighting reapplied, but recovery record could not be removed")?;
    Ok(colors)
}

fn save(path: &Path, record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).context(
        "cannot create recovery record; if one already exists, run `glance restore` first",
    )?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn load(path: &Path) -> Result<Record> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "recovery record must be a regular file"
    );
    let mut bytes = Vec::new();
    File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 65_536, "recovery record is too large");
    serde_json::from_slice(&bytes).context("invalid recovery record")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Fake {
        saved: RefCell<Vec<u8>>,
        writes: RefCell<Vec<Vec<u8>>>,
        fail_write: Cell<usize>,
        fail_readback: Cell<bool>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                saved: RefCell::new(vec![8, 1]),
                writes: RefCell::new(vec![]),
                fail_write: Cell::new(0),
                fail_readback: Cell::new(false),
            }
        }
    }
    impl Keyboard for Fake {
        fn colors(&self) -> Result<Vec<[u8; 3]>> {
            ensure!(!self.fail_readback.get(), "readback failed");
            Ok(vec![[0, 0, 0]; 10])
        }
        fn active_slot(&self) -> Result<u8> {
            Ok(0)
        }
        fn stored_profile(&self, _: u8) -> Result<Vec<u8>> {
            Ok(self.saved.borrow().clone())
        }
        fn write_profile(&self, _: u8, body: &[u8]) -> Result<()> {
            self.writes.borrow_mut().push(body.to_vec());
            ensure!(
                self.writes.borrow().len() != self.fail_write.get(),
                "write failed"
            );
            Ok(())
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "glance-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn record(&self) -> PathBuf {
            self.0.join("recovery.json")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn identity() -> Identity {
        Identity {
            product_id: 0x1400,
            serial: "test-device".into(),
        }
    }

    #[test]
    fn session_lock_excludes_concurrent_restore_and_releases_on_drop() {
        let temp = Temp::new();
        let held = Recovery::acquire(&temp.0).unwrap();
        assert!(Recovery::acquire(&temp.0).is_err());
        drop(held);
        assert!(Recovery::acquire(&temp.0).is_ok());
    }

    #[test]
    fn failed_restoration_readback_preserves_recovery_record() {
        let temp = Temp::new();
        let recovery = Recovery::acquire(&temp.0).unwrap();
        let keyboard = Fake::default();
        keyboard.fail_readback.set(true);
        assert!(
            preview(&keyboard, &identity(), &recovery, &[(0, [1, 2, 3])], || Ok(
                ()
            ))
            .is_err()
        );
        assert!(temp.record().exists());
        keyboard.fail_readback.set(false);
        restore(&keyboard, &identity(), &recovery).unwrap();
        assert!(!temp.record().exists());
    }

    #[test]
    fn restores_on_success_and_has_durable_record_before_preview() {
        let temp = Temp::new();
        let recovery = Recovery::acquire(&temp.0).unwrap();
        let keyboard = Fake::default();
        preview(
            &keyboard,
            &identity(),
            &recovery,
            &[(0, [255, 0, 0])],
            || {
                assert!(temp.record().exists());
                assert_eq!(load(&temp.record())?.baseline, vec![8, 1]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*keyboard.writes.borrow().last().unwrap(), vec![8, 1]);
        assert!(!temp.record().exists());
    }

    #[test]
    fn restores_after_partial_upload_failure_or_interrupted_preview() {
        for failed_write in [0, 1] {
            let temp = Temp::new();
            let recovery = Recovery::acquire(&temp.0).unwrap();
            let keyboard = Fake::default();
            keyboard.fail_write.set(failed_write);
            let result = preview(
                &keyboard,
                &identity(),
                &recovery,
                &[(9, [0, 0, 255])],
                || anyhow::bail!("interrupted"),
            );
            assert!(result.is_err());
            assert_eq!(keyboard.writes.borrow().len(), 2);
            assert_eq!(*keyboard.writes.borrow().last().unwrap(), vec![8, 1]);
            assert!(!temp.record().exists());
        }
    }

    #[test]
    fn keeps_record_on_restore_failure_and_rejects_other_device() {
        let temp = Temp::new();
        let recovery = Recovery::acquire(&temp.0).unwrap();
        let keyboard = Fake::default();
        keyboard.fail_write.set(2);
        assert!(
            preview(&keyboard, &identity(), &recovery, &[(1, [1, 2, 3])], || Ok(
                ()
            ))
            .is_err()
        );
        assert!(temp.record().exists());
        let mut wrong = identity();
        wrong.serial = "another".into();
        assert!(restore(&keyboard, &wrong, &recovery).is_err());
        assert_eq!(keyboard.writes.borrow().len(), 2);
        keyboard.fail_write.set(0);
        restore(&keyboard, &identity(), &recovery).unwrap();
        assert!(!temp.record().exists());
    }

    #[test]
    fn existing_record_blocks_new_preview_without_writes() {
        let temp = Temp::new();
        let recovery = Recovery::acquire(&temp.0).unwrap();
        let keyboard = Fake::default();
        fs::write(temp.record(), b"keep me").unwrap();
        assert!(
            preview(&keyboard, &identity(), &recovery, &[(1, [1, 2, 3])], || Ok(
                ()
            ))
            .is_err()
        );
        assert!(keyboard.writes.borrow().is_empty());
        assert_eq!(fs::read(temp.record()).unwrap(), b"keep me");
    }

    #[test]
    fn restoration_respects_new_saved_settings() {
        let temp = Temp::new();
        let recovery = Recovery::acquire(&temp.0).unwrap();
        let keyboard = Fake::default();
        preview(&keyboard, &identity(), &recovery, &[(2, [1, 2, 3])], || {
            *keyboard.saved.borrow_mut() = vec![8, 1, 32, 64];
            Ok(())
        })
        .unwrap();
        assert_eq!(
            *keyboard.writes.borrow().last().unwrap(),
            vec![8, 1, 32, 64]
        );
    }

    #[test]
    fn refuses_malformed_or_unknown_recovery_record() {
        let temp = Temp::new();
        let recovery = Recovery::acquire(&temp.0).unwrap();
        let keyboard = Fake::default();
        for record in [
            "{}",
            "not json",
            r#"{"version":2,"device":{"product_id":5120,"serial":"test-device"},"slot":0,"baseline":[8,1]}"#,
        ] {
            fs::write(temp.record(), record).unwrap();
            assert!(restore(&keyboard, &identity(), &recovery).is_err());
            assert!(keyboard.writes.borrow().is_empty());
        }
    }
}
