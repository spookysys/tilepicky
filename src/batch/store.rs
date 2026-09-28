// SPDX-License-Identifier: GPL-3.0-only
//! The library book owns one job. The coordinator and the UI change separate fields under the same lock.

use super::*;

#[derive(Default, Serialize, Deserialize)]
struct Record {
    job: Option<Job>,
    #[serde(default)]
    control: runner::Control,
}

fn decode(value: Option<Value>) -> Result<Record, String> {
    value.map(serde_json::from_value).transpose().map(|r| r.unwrap_or_default())
        .map_err(|e| format!("Cannot read the library batch: {e}"))
}

fn update<T>(root: &Path, change: impl FnOnce(&mut Record) -> Result<T, String>) -> Result<T, String> {
    crate::sidecar::update_book(root, |book| {
        let mut record = decode(book.ai_batch.take())?;
        let result = change(&mut record)?;
        book.ai_batch = Some(serde_json::to_value(record).map_err(|e| e.to_string())?);
        Ok(result)
    })
}

pub(super) fn load(root: &Path) -> Result<Option<Job>, String> {
    Ok(decode(crate::sidecar::load_book(root)?.ai_batch)?.job)
}

pub(super) fn save(root: &Path, job: &Job) -> Result<(), String> {
    update(root, |record| { record.job = Some(job.clone()); Ok(()) })
}

pub(super) fn start(root: &Path, job: &Job) -> Result<(), String> {
    update(root, |record| {
        if record.job.as_ref().is_some_and(|j| !j.done()) { return Err("This library already has an outstanding job.".into()); }
        record.job = Some(job.clone());
        Ok(())
    })
}

pub(super) fn clear(root: &Path) -> Result<(), String> {
    update(root, |record| {
        if record.job.as_ref().is_some_and(|j| !j.done()) { return Err("The library job has not finished.".into()); }
        // Keep the record so a legacy journal cannot become active again.
        record.job = None;
        Ok(())
    })
}

pub(super) fn control(root: &Path) -> Result<runner::Control, String> {
    Ok(decode(crate::sidecar::load_book(root)?.ai_batch)?.control)
}

pub(super) fn set_control(root: &Path, control: &runner::Control) -> Result<(), String> {
    update(root, |record| { record.control = control.clone(); Ok(()) })
}

/// Copy first, then retire the old journal. A crash between these writes keeps the library record authoritative.
pub(super) fn migrate(root: &Path, legacy: &Path) -> Result<(), String> {
    if !legacy.join("state.json").exists() { return Ok(()); }
    let _legacy_lock = runner::lock_file(&legacy.join("writer.lock"))?;
    crate::sidecar::update_book(root, |book| {
        if book.ai_batch.is_none() {
            let job: Option<Job> = crate::storage::read(&legacy.join("state.json"))?;
            let control = crate::storage::read(&legacy.join("control.json"))?;
            book.ai_batch = Some(serde_json::to_value(Record { job, control }).map_err(|e| e.to_string())?);
        } else { decode(book.ai_batch.clone())?; }
        Ok(())
    })?;
    std::fs::remove_file(legacy.join("state.json")).map_err(|e| format!("Cannot retire the old batch journal: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(root: &Path) -> Job {
        image::RgbaImage::new(2, 2).save(root.join("sheet.png")).unwrap();
        prepare(&Index::scan(root, [16, 16]), super::super::tests::provider(Kind::Gemini), "test".into(), Scope::All).unwrap()
    }

    #[test]
    fn a_library_owns_one_job_and_keeps_it_when_moved() {
        let f = crate::storage::tests::Folder::new();
        let root = f.0.join("before"); std::fs::create_dir(&root).unwrap();
        let job = job(&root); start(&root, &job).unwrap();
        assert!(start(&root, &job).is_err()); assert!(clear(&root).is_err());
        let moved = f.0.join("after"); std::fs::rename(&root, &moved).unwrap();
        assert_eq!(load(&moved).unwrap().unwrap().sheets[0].rel, "sheet.png");
        assert!(start(&moved, &job).is_err());
    }

    #[test]
    fn migration_preserves_accepted_work_and_cannot_resurrect_cleared_work() {
        let f = crate::storage::tests::Folder::new(); let root = &f.0;
        let mut job = job(root);
        job.sheets[0].taken = true;
        job.groups.push(Group { sheets: vec![0], remote: Remote::Waiting("batches/accepted".into()), recovery: None, tracking: Tracking::default() });
        let legacy = root.join("legacy");
        crate::storage::write_private(&legacy.join("state.json"), &job).unwrap();
        crate::storage::write_private(&legacy.join("control.json"), &runner::Control { revision: 7, mode: Mode::Paused }).unwrap();
        migrate(root, &legacy).unwrap();
        assert!(!legacy.join("state.json").exists());
        assert_eq!(load(root).unwrap().unwrap().groups[0].remote, Remote::Waiting("batches/accepted".into()));
        assert!(control(root).unwrap().mode == Mode::Paused);
        // Simulate a crash after the library write and before the old journal was removed.
        crate::storage::write_private(&legacy.join("state.json"), &job).unwrap();
        job.groups[0].remote = Remote::Done; job.sheets[0].error = "Cancelled".into(); save(root, &job).unwrap();
        clear(root).unwrap(); migrate(root, &legacy).unwrap();
        assert!(load(root).unwrap().is_none());
    }

    #[test]
    fn migration_refuses_an_active_legacy_writer_or_a_damaged_book() {
        let f = crate::storage::tests::Folder::new(); let root = &f.0;
        let job = job(root); let legacy = root.join("legacy");
        crate::storage::write_private(&legacy.join("state.json"), &job).unwrap();
        let owner = runner::lock_file(&legacy.join("writer.lock")).unwrap();
        assert!(migrate(root, &legacy).is_err()); drop(owner);
        std::fs::write(root.join(crate::sidecar::BOOK), "broken").unwrap();
        assert!(migrate(root, &legacy).is_err());
        assert!(legacy.join("state.json").exists());
    }

    #[test]
    fn progress_commands_and_grid_writes_preserve_each_other() {
        let f = crate::storage::tests::Folder::new(); let root = &f.0; let job = job(root);
        start(root, &job).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for n in 1..=20 { set_control(root, &runner::Control { revision: n, mode: Mode::Paused }).unwrap(); }
            });
            scope.spawn(|| {
                let mut snapshot = job.clone();
                for n in 1..=20 { snapshot.last_response_ms = n; save(root, &snapshot).unwrap(); }
            });
            scope.spawn(|| {
                for n in 1..=20 { crate::sidecar::store_tile(root, [n, n]).unwrap(); }
            });
        });
        let book = crate::sidecar::load_book(root).unwrap();
        assert_eq!(book.tile, Some(crate::sidecar::Pair::One(20)));
        assert_eq!(control(root).unwrap().revision, 20);
        assert_eq!(load(root).unwrap().unwrap().last_response_ms, 20);
    }
}
